# UB-REVIEW-SPEC-0003 — intelligent-ci gate surface

Status: authored 2026-06-06 (release surface spec wave, docs-only).
Umbrella: [UB-REVIEW-SPEC-0001](UB-REVIEW-SPEC-0001-release-surface.md).
Maturity: production on this repository — `ub-review/gate` is the sole
required PR check on `EffortlessMetrics/ub-review` (roadmap item 26), with
live red/green receipts. The configured ripr threshold production-evaluates
since #335 (closing #316) and has blocked two real PRs (#342, #346). Named
gaps: `[gate].synchronize_mode` is inert (#306), and tool-gate receipts stop
at counts — blocking findings are not identifiable from downloaded artifacts
alone (#347). Neither gap is papered over below.

## Purpose

Define the surface a repository relies on when it makes `ub-review/gate` its
required PR check: which repo-written policy can turn the check red, the
`gate_outcome.json` contract that records why, the exit-code and enforcement
chain that turns the artifact into a verdict, and the posting policy that
keeps the gate honest on every head SHA without taxing the PR thread.

## User question

```text
Can this replace my required PR CI gate?
```

Yes, with receipts: since 2026-06-06 branch protection on this repository's
`main` requires exactly one status check, `ub-review/gate`; the former
`ci.yml`/`coverage.yml` jobs run inside it as `[[proof.required]]` tasks and
required tools (docs/ROADMAP.md item 26, fold PRs #300/#301). A deliberately
broken required proof turned the check red with a receipted reason and the
revert turned it green (PR #305; runs `27069200002` / `27069711280`).

## Lifecycle moment

Every PR pass. The gate workflow triggers on `opened`, `reopened`,
`ready_for_review`, and `synchronize` plus manual `workflow_dispatch`
(.github/workflows/ub-review-gate.yml): a required check must report on every
head SHA. Posting the grouped review is a separate, narrower decision
(`[gate].post_review_on`); the verdict is not.

## Consumer

```text
branch protection      one required status check: ub-review/gate
the PR author          check verdict; on red, the reason ids in the check log
the maintainer         review/gate_outcome.json and its receipt pointers
automation             gate-outcome-path action output (action.yml outputs)
```

The check name comes from `[gate].required_check`, default `ub-review/gate`
(src/config.rs `GateConfig`). Branch protection is configured manually today
(API or UI; verified on this repo via
`GET /repos/EffortlessMetrics/ub-review/branches/main/protection`,
app-pinned, `strict: false` — docs/ROADMAP.md item 26). The future `setup-ci`
command (roadmap item 28, not implemented) will spell out the exact
required-checks change in a migration PR but never mutate branch protection
itself. Rollback for this repo is recorded under roadmap item 26: restore
`ci.yml`/`coverage.yml` from git history and re-add their contexts to
`required_status_checks.contexts`.

## Inputs

Repo policy in root `.ub-review.toml` (the same path consumer repos use —
this repository's own file is the production example):

- `[[proof.required]]` — the required proof stream. Fields per entry: `id`,
  `languages`, `diff_classes`, `command`, `reason`, `cost`, `timeout_sec`,
  `required`, `enabled` (src/config.rs `RequiredProofPolicy`). A policy
  matches a run when its `languages` and `diff_classes` lists match the
  classified diff; `"all"` is the wildcard. Scoping is real: on PR #305's
  TOML-only diff, `policy-check` (`languages = ["all"]`) matched while the
  Rust-scoped `cargo-check`/`cargo-doc` did not (`required_proof.matched: 1`).
- The `intelligent-ci-policy` lane constraint: a proof request counts toward
  the gate only when `required = true` AND its lane is exactly
  `intelligent-ci-policy` — the lane the policy expander assigns to
  `[[proof.required]]` entries (src/main.rs `proof_request_is_gate_required`,
  `REQUIRED_PROOF_POLICY_LANE`). Model lanes can also mark proof requests
  required; those never gate-block. Model output cannot reach the verdict.
- `[tools.<id>]` with `required = true` — required sensors. On this repo:
  cargo-fmt, cargo-check, cargo-test, cargo-clippy, cargo-doc,
  artifact-verifier, cargo-allow, ripr, unsafe-review, ast-grep, actionlint
  (.ub-review.toml). A required sensor whose trigger matched and whose
  evidence is missing, skipped, failed, or timed out blocks the gate — in
  `intelligent-ci` mode only.
- `[tools.<id>.gate]` thresholds — opt-in per tool. Fields: `scope`
  (`"on-diff"` is the only scope semantics that exist; other values are
  stripped at load with a `PolicyError` receipt) and `max_new_unsuppressed`
  (src/config.rs `ToolGatePolicy`). Only tools with a configured `gate`
  entry produce tool-gate outcomes; tools without one cannot redden the gate.
  Production status (#316 closed by #335): this repo configures
  `[tools.ripr.gate] max_new_unsuppressed = 0` and the threshold evaluates
  on every run that triggers the ripr sensor — the sensor runs
  `ripr check --diff --mode ready --format badge-json` (ripr pinned 0.8.0
  in the install script and doctor) and persists the verbatim stdout as
  `sensors/ripr/gate-decision.json`; the threshold reads
  `counts.unsuppressed_exposure_gaps`. First production evaluation: run
  27077206713. First production blocks: PR #342 (run 27078623035,
  new_unsuppressed=1) and PR #346 (run 27080588485, new_unsuppressed=2),
  each answered with an exact-oracle test, an upstream ripr-swarm issue,
  and an owned suppression in `.ripr/suppressions.toml`. A configured but
  never-evaluated required tool gate raises a loud unevaluated-gate alarm
  in the running summary. Known receipt-depth gap: badge-json carries
  counts only, so identifying *which* findings blocked requires a local
  ripr re-run (#347).
- `[gate]` — `required_check`, `target_minutes`, `hard_timeout_minutes`,
  `post_review_on` (default `["opened", "ready_for_review"]`),
  `synchronize_mode` (declared, defaulted to `gate-only`, read by no
  functional code — #306; posting on quiet passes is governed solely by
  `post_review_on`), and `blocking` (src/config.rs `GateConfig`).
- `[gate.blocking]` opt-ins, both default `false` (src/config.rs
  `GateBlockingPolicy`): `required_proof_unproven` blocks when a matched
  `[[proof.required]]` produced no passing receipt (missing receipt or any
  skipped class); `tool_gate_missing_evidence` blocks when a configured
  threshold on a *required* tool could not be evaluated. Defaults preserve
  pre-policy gate behavior; repos opt into stricter posture explicitly.

Run inputs: action `mode: intelligent-ci` (action.yml; legacy `review-direct`
is an alias of `review-byok`, not of `intelligent-ci` — it never enforces
under `fail-on-gate` `auto`), `fail-on-gate` (`auto`/`true`/`false`),
`run-pass: auto` resolved
from `github.event.action` via `UB_REVIEW_GITHUB_EVENT_ACTION` (action.yml).
Model keys are optional inputs; their absence degrades the review, never the
verdict.

Note: this repository's `.ub-review.toml` also carries a `[providers]`
section. That section is reserved and unwired (umbrella 0001; src/config.rs)
— provider selection on the gate workflow comes from action inputs
(`provider-policy: primary-with-fallback` in
.github/workflows/ub-review-gate.yml), not from the config block.

## Output artifact / user surface

```text
review/gate_outcome.json     the verdict artifact (ub-review.gate_outcome.v1)
ub-review/gate check         green/red on every PR pass
grouped PR review            posting passes only; one review, line comments
                             plus a decision-led body
gate-outcome-path            action output resolving to
                             <out>/review/gate_outcome.json (action.yml)
running-summary.md           appended to the GitHub step summary
```

`gate_outcome.json` is written only under `review/`; its path is not yet
configurable.

## Required fields

`gate_outcome.json` (schema `ub-review.gate_outcome.v1`, src/main.rs
`GateOutcome`; minimum shape in docs/adr/0002):

```text
schema                    "ub-review.gate_outcome.v1" exactly
conclusion                "pass" | "fail"
terminal_status           from ub-review.terminal_state.v1
reasons[]                 {kind, id, detail, receipt}
required_proof            {matched, passed, failed, skipped}
tool_gates                {evaluated, passed, failed}
evidence_gaps_blocking    count
evidence_gaps_advisory    count
```

Reason kinds (src/main.rs gate outcome construction; docs/adr/0002):

```text
required-proof      a matched [[proof.required]] task failed (head_failed,
                    timed_out)
tool-gate           a configured [tools.*.gate] threshold was evaluated and
                    exceeded
required-sensor     a required sensor's matched trigger produced no evidence
                    (receipt-absent, failed, skipped, timed-out);
                    intelligent-ci mode only
blocking-finding    today emitted only by the two [gate.blocking] evidence
                    opt-ins (required_proof_unproven,
                    tool_gate_missing_evidence); per-finding-class
                    blocking = true markers are ADR 0002 intent, deferred
                    until findings carry deterministic per-class receipts
                    (src/config.rs GateBlockingPolicy)
policy              the repo wrote a malformed policy section; parse errors
                    are receipted gate failures, not silent defaults
internal            declared in the ADR 0002 schema; no run today writes it,
                    because internal failures abort before gate construction
                    and surface as a missing artifact the fail-closed
                    gate-check rejects
```

The two `[gate.blocking]` opt-ins surface as `blocking-finding` reasons, not
as `required-proof`/`tool-gate` kinds — automation keying on reason kinds
must expect that.

Every `fail` reason carries a receipt pointer into existing artifacts: proof
reasons point into `review/proof_receipts.json#<id>`, required-sensor reasons
point at `sensors/<id>/ub-review-sensor-status.json` (or
`review/terminal_state.json` when the receipt itself is absent), tool-gate
reasons point at `review/tool-gate-outcomes.json#<tool>`, whose entry's
`source_artifacts` chain to `sensors/<tool>/gate-decision.json` when the
sensor produced one. A red gate with no receipt is a bug in the gate, not a
finding (docs/adr/0002).

Proof receipt classification (src/main.rs `required_proof_receipt_class`):

```text
passed    head_passed | discriminating
failed    head_failed | timed_out            blocks unconditionally
skipped   skipped_budget | skipped_profile |
          non_discriminating | base_patch_failed
                                             blocks only when
                                             required_proof_unproven = true
```

Tool gate outcome values: `passed`, `failed`, `missing_evidence`,
`not_evaluated`. Only `failed` blocks unconditionally.

## Advisory vs blocking behavior

Blocks (turns `ub-review/gate` red):

```text
a matched [[proof.required]] task that failed
an evaluated [tools.*.gate] threshold that was exceeded
a required sensor evidence gap whose trigger matched   (intelligent-ci only)
a policy parse error on a policy section the repo wrote
an internal failure (aborts the run; the check goes red via the
  missing-artifact fail-closed path, not via a kind: internal reason)
required-proof-unproven / tool-gate-missing-evidence    (only when the repo
                                                         opted in via
                                                         [gate.blocking];
                                                         surfaces as
                                                         blocking-finding)
```

Never reds (docs/adr/0002; src/main.rs gate outcome logic never reads
provider state):

```text
model or provider failures, including fallback use and missing model keys
optional (non-required) missing evidence
successful tool status
artifact-only observations and lane metadata
a failed-to-review terminal status with no blocking reason
  (model availability gaps stay visible as evidence_gaps_advisory)
```

Required vs advisory is mode-sensitive: required-sensor gaps block only in
`intelligent-ci`; `review-byok` treats the same gaps as advisory. The counts
are explicit in the artifact (`evidence_gaps_blocking` /
`evidence_gaps_advisory`). Live receipt: on PR #305's red run a real coverage
sensor failure (exit 101) stayed advisory (`evidence_gaps_advisory: 1`,
coverage is `required = false` here) and did not flip the verdict; the sensor
recovered on the green run without intervention.

Posting policy (`[gate].post_review_on`): the gate verdict lands on every
pass, but the grouped review posts only on passes whose `pull_request` event
action is listed — `opened` and `ready_for_review` on this repo. Synchronize
and reopened passes run the full gate and stay quiet, writing
`review/github-review-skip.json` with `skipped_pass_policy` (src/main.rs
`pass_policy_permits_review_post`, honored for synchronize/reopened since PR
#304). Manual runs are explicit operator requests and bypass the pass list;
catch-all `pull_request_other` passes never post. Quiet-pass proof on PR
#307: the opened pass (run `27070590534`) passed 3/3 matched required proofs
and posted one grouped review with a line-local finding that was then
applied; the synchronize pass (run `27070880408`) concluded `pass` and
posted nothing, with the skip receipt recorded. PR #308 is the docs-only
closure record of that verified state.

PR-visible content contract: one grouped review per posting pass — validated
inline comments anchored to diff lines (capped by `max-inline-comments`,
default 8, action.yml) plus a decision-led body. Allowed body content:
decision, confirmed findings, verification questions, proof results,
refutations, parked follow-ups, specific evidence gaps. No lane rosters,
provider/sensor status tables, command logs, or generic residual risk —
those stay in artifacts (docs/REVIEW_BODY_CONTRACT.md; `[review_body]`
table policies in .ub-review.toml).

## Fail-closed behavior

The gate check recognizes exactly the string `pass` in an artifact with
exactly the schema `ub-review.gate_outcome.v1` (src/main.rs
`cmd_gate_check`). When enforcement is on:

```text
missing gate_outcome.json          failure
unexpected or missing schema       failure, named in the error
conclusion "fail"                  failure, reason ids listed
null / missing / case-drifted
conclusion ("Fail")                failure naming the unexpected value
```

A corrupted or future-incompatible artifact can never silently pass.

`fail-on-gate` resolution (src/cli.rs `FailOnGate::resolved`): `true` and
`false` are literal; `auto` resolves to `true` only for `intelligent-ci` —
`review-byok` stays non-blocking by default. This is the only place in the
product where `auto` means enforce.

Single-source enforcement in the action (action.yml): the run step always
invokes the binary with `--fail-on-gate false` so a failing gate cannot skip
review posting; the final "Enforce gate outcome" step then calls
`ub-review gate-check --gate-outcome <out>/review/gate_outcome.json
--fail-on-gate <input> --mode <input>`. `gate-check` is the single source of
the `FailOnGate::resolved` semantics; the workflow step only forwards inputs
and never re-implements the resolution in bash. Direct CLI runs get the same
contract from `run` itself: it exits non-zero when the conclusion is `fail`
and the resolved flag is true (src/main.rs run completion).

Policy parse errors fail closed in the repo's favor: malformed keys in
`[gate]`, `[proof]`, `[review_body]`, `[tools.*]`, or `[tools.*.gate]` are
stripped per-key with `PolicyError` receipts, and those receipts become
`kind = "policy"` blocking reasons — a policy the repo wrote is never
silently replaced by defaults (src/config.rs `sanitize_policy_sections`;
roadmap item 24 acceptance).

## Trust boundary / non-claims

```text
The gate decides; models investigate.
Model output never feeds the verdict directly.
Missing evidence is recorded as missing evidence, never as clean evidence.
A red gate without a receipt pointer is a gate bug.
```

Never claim: proves code correct, proves UB-free, replaces all security
tooling, runs every possible test (umbrella 0001). The gate enforces exactly
the proofs and thresholds the repo configured — nothing more.

Honest current-state limits a consumer must know:

- `[tools.ripr.gate]` enforces in production (#335), but its receipt depth
  is counts-only: `gate-decision.json` is badge-json, so a tool-gate red
  names the count, not the findings — diagnosis requires a local
  `ripr check` re-run until #347 lands per-finding detail in the artifacts.
- ripr suppressions are line-keyed upstream (ripr-swarm#1053): an entry in
  `.ripr/suppressions.toml` can go dead or silently transfer to unrelated
  code when edits move declarations across lines. Re-verify suppression
  targets whenever the suppressed file changes; this repo records target
  expressions and re-verify notes in each entry's reason.
- ripr reach analysis has known false-negatives (ripr-swarm#1052,
  ripr-swarm#1054): an exactly-asserted change can still report an exposure
  gap, forcing an owned suppression for a non-defect. The governance loop —
  strengthen what is genuine, file the tool half upstream, suppress with a
  receipt — is the supported answer; do not loosen the threshold.
- `[gate].synchronize_mode` is declared and defaulted but consumed by no
  functional code (#306). Do not configure it expecting behavior.
- `model-mode: off` is the intended zero-key product tier (docs/adr/0002
  "Out-of-the-box posture"); this repo's gate runs with model lanes on, so
  the zero-key tier's gate posture is design intent validated by the
  model-failure-never-reds rule, not by a dedicated production dogfood.
- Heavy witnesses (Miri, ASAN, mutation, leased coverage) run only behind
  explicit `allow-heavy` workflow policy; this repo leases coverage on every
  PR by choice (.ub-review.toml `[tools.coverage]`,
  .github/workflows/ub-review-gate.yml).

## Validation commands

```bash
cargo test --bin ub-review --locked            # gate outcome, gate-check, and
                                               # pass-policy contracts are
                                               # pinned in the inline tests
ub-review run --mode intelligent-ci --config .ub-review.toml \
  --out target/ub-review                       # exits non-zero on a red gate
ub-review gate-check \
  --gate-outcome target/ub-review/review/gate_outcome.json \
  --fail-on-gate auto --mode intelligent-ci    # the enforcement step, locally
python scripts/verify-bun-review-artifacts.py target/ub-review \
  --expected-review-profile ub-review-self --expected-repo-kind ub-review
```

Live receipts (do not re-prove; cite): red run `27069200002`, green run
`27069711280` (PR #305); quiet-pass runs `27070590534` / `27070880408`
(PR #307, recorded via PR #308); first tool-gate evaluation run
`27077206713` (#335); production tool-gate blocks run `27078623035`
(PR #342) and run `27080588485` (PR #346); branch-protection state under
docs/ROADMAP.md item 26.

## Implementation PR slices

This spec is docs-only. Open gate-surface work it routes:

```text
#316   DONE (#335): gate-decision.json from ripr badge-json stdout, parser
       on counts.unsuppressed_exposure_gaps, ripr/unsafe-review version
       pins in install script + doctor, unevaluated-required-gate alarm
#347   tool-gate receipts stop at counts: persist per-finding exposure-gap
       detail in sensor artifacts so a block is diagnosable from the
       artifact tree alone
#306   wire [gate].synchronize_mode to real routing semantics or delete it
item 27  rust-test-proof profile; one required check per onboarded repo,
         model-mode off as a supported tier
item 28  setup-ci migration PR mode: the branch-protection change spelled
         out in docs, never applied by the PR
```

## Release note claim

```text
ub-review provides a repo-configured intelligent CI gate.
```

Concretely claimable: `ub-review/gate` is the only required PR check on
`EffortlessMetrics/ub-review`; required fmt/check/test/clippy/doc/policy
proof runs inside it; a deliberately broken required proof turned the check
red with a receipted reason and the revert turned it green; the configured
ripr threshold has blocked two real PRs (#342, #346) with tool-gate reasons
and receipt chains. Not claimable: that a tool-gate red names the blocking
findings in artifacts (counts only, #347), or that the gate proves code
correct.

## The six reliance questions

What can a user rely on?
The `ub-review.gate_outcome.v1` schema and its required fields; the reason
kinds; receipt pointers on every fail reason; the exact-`pass` fail-closed
gate check; `fail-on-gate=auto` meaning enforce only in `intelligent-ci`;
a verdict on every head SHA; quiet synchronize/reopened passes leaving a
truthful `skipped_pass_policy` receipt.

What can break the gate?
Only repo-written policy outcomes: failed required proof from
`[[proof.required]]` (lane `intelligent-ci-policy`), an exceeded
`[tools.*.gate]` threshold, required-sensor evidence gaps (intelligent-ci
mode), policy parse errors, internal failures via the missing-artifact
fail-closed path — plus the two `[gate.blocking]` opt-ins if the repo
enables them (surfacing as `blocking-finding` reasons).

What is only advisory?
Everything model- or provider-shaped, optional sensor gaps, non-required
tool results, `missing_evidence` tool-gate outcomes under default policy,
and `evidence_gaps_advisory` generally.

What is visible in the PR?
The required check verdict on every pass; on `post_review_on` passes, one
grouped review: capped diff-anchored line comments plus a decision-led body
with no rosters or status tables.

What is artifact-only?
`review/gate_outcome.json` and its receipt chain (`proof_receipts.json`,
`proof/<id>/head/stderr.txt`, sensor status receipts,
`tool-gate-outcomes.json`), `github-review-skip.json` on quiet passes, and
all diagnostics the body contract excludes.

What does success look like in ten minutes?
Break a required proof on purpose — e.g. delete a required `reason` field
from a `policy/ci-lanes.toml` lane entry, as PR #305 did — and open a PR.
The gate goes red; `review/gate_outcome.json` records `conclusion: fail`
with exactly one reason (`kind: required-proof`, `id: policy-check`,
`receipt: review/proof_receipts.json#proof-build-58fe49dac211` on the live
run); the receipt's stderr leaf
(`proof/proof-build-58fe49dac211/head/stderr.txt`) names the missing field.
Revert, push, and the check goes green with `required_proof.passed`
incremented and zero blocking gaps. Red has a receipt chain; green has a
count — never a vibe.
