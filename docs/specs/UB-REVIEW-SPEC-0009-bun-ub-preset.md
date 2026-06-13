# UB-REVIEW-SPEC-0009 - bun-ub preset surface

Status: authored 2026-06-06 (release surface spec wave, docs-only). Maturity
per UB-REVIEW-SPEC-0001: production at the pinned SHA; consumer contract live.
This spec documents what the Bun UB hunt actually consumes today. Intent is
marked as intent; inert knobs are named with their issues.

## Purpose

`bun-ub` is the first production preset and the calibration target for the
whole product. It packages the Bun UB hunt posture - ten MiniMax lanes plus a
refuter over a deterministic evidence packet, six sensors (five configured
in profiles/bun-ub-v0.toml plus the builtin-enabled actionlint), focused
red/green proof, and one grouped PR review - behind a single `preset: bun-ub`
action input pinned to a verified commit SHA. The preset exists so the Bun
fork never vendors the Rust runner and never floats on `main`
(docs/ACTION_CONSUMER_BUN.md).

The hunt it serves is invariant-driven, not surface-driven. The stable-bytes
invariant from docs/BUN_UB_HUNT.md:

```text
Rust/native code must not retain or later materialize JS-owned bytes after JS
can resize, detach, alias, mutate, race, or reenter.
```

Lane focuses, the hardcoded tokmd preset, and the calibration test fixtures
(for example "keeps stable bytes after getter reentry" in the inline tests in
src/main.rs) all encode this invariant.

## User question

How does the Bun UB hunt use this gate?

## Lifecycle moment

Two posting passes per PR on the Bun fork: `pull_request.opened` (drafts
included) and `pull_request.ready_for_review`. No `synchronize` trigger by
default - the Bun workflow does not subscribe to it
(examples/bun/.github/workflows/ub-review-packet.yml), and the gh-runner
runtime profile declares the same trusted-repo pass triggers with
`synchronize = false` (runtime/gh-runner.toml). Posting on any pass is
governed solely by `[gate].post_review_on`, default
`["opened", "ready_for_review"]` (src/config.rs); passes outside that list
run fully but record `skipped_pass_policy` instead of posting (PR #304).
Legacy `[gate].synchronize_mode` is stripped with a deprecation
`PolicyError` (#306); the Bun preset does not rely on it.

## Consumer

Three consumers, in order of authority:

1. The Bun fork workflow `EffortlessSteven/bun` `UB Review Packet`, copied
   from examples/bun/.github/workflows/ub-review-packet.yml. It runs the
   action, posts the grouped review with the scoped `github.token`, and
   uploads `target/ub-review` as the durable artifact.
2. The packet verifier scripts/verify-bun-review-artifacts.py, run against
   the downloaded Bun artifact. It is the release proof for any pin advance.
3. The human UB hunter, reading the packet in the order docs/BUN_UB_HUNT.md
   prescribes (running-summary.md, review/review.md, proof_receipts.json,
   lanes/tests.md, lanes/ub.md, lanes/source-route.md, input/diff.patch) and
   recording calibration in docs/calibration/bun-ub-review-ledger.md.

## Inputs

### The pinned-SHA consumer contract

The locked baseline (docs/ROADMAP.md "Current locked baseline"):

```text
EffortlessMetrics/ub-review@804d198b5a15a0df94bb4f43750dba71165916cd
review-byok
MiniMax-only, 10 Bun lanes plus refuter
one grouped Pull Request Review
full artifact packet
missing evidence reported as missing evidence, never as clean evidence
```

The pin moves only after (a) the local verifier passes and (b) a Bun consumer
workflow run on the candidate SHA uploads a valid packet that the verifier
also passes (docs/GH_RUNNER_BUN.md, docs/ACTION_CONSUMER_BUN.md). Live
receipts for the current contract:

- Smoke proof: `EffortlessSteven/bun#29`, workflow `UB Review Packet`, run
  `26843669614` attempt 2, artifact `ub-review-packet-29`, digest
  `sha256:2ec165d6487ef1f9c78999e0cf25ce6be4ead78762c68a8acaf4f5b3a9c6ac24`,
  strict verifier passed with 10 expected MiniMax lanes plus refuter ok,
  3 inline comments, 0 off-diff comments, 0 missing sensor or model evidence,
  post HTTP 200 (docs/ROADMAP.md, docs/calibration/bun-ub-review-ledger.md).
  Attempt 1 of the same run hit a MiniMax preflight HTTP 500 and correctly
  recorded missing model evidence instead of a clean review; that behavior is
  part of the release contract.
- Pin advance: `EffortlessSteven/bun#49`, run `26991938752`, artifact
  `ub-review-packet-49`, advanced after the artifact-only PR body guard
  landed (ub-review PR #251); terminal state `sufficient` with
  `github-review-skip.json` and `post-result.json` recording an artifact-only
  skip instead of no-value review text
  (docs/calibration/bun-ub-review-ledger.md).

### profiles/bun-ub-v0.toml (review posture)

`preset: bun-ub` resolves to the bundled profiles/bun-ub-v0.toml (action.yml
config resolution; `DEFAULT_REVIEW_PROFILE = "bun-ub-v0"` in src/main.rs).
The profile sets:

- `review_profile = "bun-ub-v0"`, `profile = "gh-runner"`.
- Posting policy: `posting_engine = "artifact"`, `custom_poster = false`,
  `ban_standalone_approval = true`, `require_zero_finding_audit = true`,
  `enable_default_lanes = true`.
- Review body guards: `include_successful_lane_table = false`,
  `include_provider_table = "on_failure"`,
  `include_sensor_table = "on_failure"`,
  `include_execution_summary = "none"`.
- Tools enabled with budgets: tokmd (always, 300s, 192MB), cargo-allow
  (source-exception-changed, 120s, 64MB), ripr
  (rust-behavior-or-tests-changed, 240s, 128MB), unsafe-review
  (unsafe-or-native-risk-changed, 240s, 128MB), ast-grep (source-changed,
  60s, 64MB).
- Tools disabled to keep the first GH-runner pass lean: semgrep, osv-scanner,
  cargo-audit, cargo-deny, shellcheck, cppcheck, zizmor, gitleaks. Missing
  commands would already be explicit missing evidence; disabling them keeps
  the plan quieter (comment in profiles/bun-ub-v0.toml).
- No `[tools.<id>.gate]` thresholds and no `[gate].blocking` opt-ins: nothing
  in this profile can redden a gate. The ripr threshold on the ub-review
  repo itself production-enforces since #335 (#316 closed; blocks on
  PR #342/#346); Bun v0 still does not depend on it.

### Model lanes: ten plus refuter, MiniMax-only

The Bun workflow passes `lane-width: '10'` and `provider-policy:
minimax-only`. At width 10 with a `SourceUb` diff class, lane planning routes
to the ten standard MiniMax lanes (src/main.rs `standard_minimax_lanes`):

```text
ub-memory-lifetime   ub-active-view     ub-worker-handoff
tests-red-green      tests-oracle       source-route
sibling-paths        architecture       security
opposition
```

Pass 2 is the observation-aware refuter (`run_refuter_pass` in src/main.rs),
which classifies validated inline candidates before the final compile and
demotes inline candidates when it is unavailable. Honest routing caveats:

- The ten-lane roster applies to `SourceUb` diffs. Other diff classes route
  to smaller rosters (`source_general_lanes`, `tests_only_lanes`, and
  workflow/tooling rosters selected by changed surface); docs-only and
  artifact-only-smoke diffs get zero model lanes (src/main.rs
  `review_lanes_for_width`). The Bun workflow's
  `paths-ignore` keeps docs-only PRs out entirely.
- The six-lane set in src/builtin.rs (`ub`, `source-route`, `tests`, `arch`,
  `opposition`, `security`) is the width-6 default, not the Bun v0 roster.
- `minimax-only` means there is no fallback provider: a MiniMax outage
  degrades lanes to missing model evidence, never to a different model.
  Runtime fallback retry and wave shedding exist for policies that configure
  a fallback, but they do not affect minimax-only Bun v0.

Model budgets from the workflow: `model-timeout-sec: '300'`,
`model-concurrency: '8'`, `max-model-calls: '14'`, matching
`STANDARD_MODEL_CONCURRENCY` and `STANDARD_MAX_MODEL_CALLS` (src/main.rs).
MiniMax M3 runs through the Anthropic-compatible endpoint
(`minimax-provider-kind: anthropic`) with explicit prompt caching; the cache
manifest records provider minimax, endpoint anthropic-messages.

Secrets contract: the action maps `secrets.MINIMAX_API_KEY` to
`UB_REVIEW_MINIMAX_API_KEY` and `secrets.OPENCODE` to
`UB_REVIEW_OPENCODE_API_KEY`; OpenCode is reserved for later direct-provider
canary/deep modes and `ub-review` does not invoke the OpenCode agent harness.
`FACTORY_API_KEY` is not an action input for this preset and stays out of the
Bun workflow (docs/GH_RUNNER_BUN.md, docs/ACTION_CONSUMER_BUN.md).

### gh-runner runtime profile (box constraints)

runtime/gh-runner.toml supplies box budgets, separate from the review
profile:

- Guards: `min_free_mem_mb = 1500`, `min_free_disk_mb = 4000`,
  `max_load_1m = 6.0`.
- Budgets: artifact 750MB, scratch 4000MB, default timeout 1800s, hard
  timeout 3600s.
- Proof caps: `proof_max_focused_test_files = 3` (a PR touching more test
  files is silently capped, not errored), `proof_max_focused_tests = 1`,
  per-command 300s, total 600s, 2 CPU, 2048MB, `proof_network = false`
  (focused proofs cannot fetch anything), `proof_scratch = true`.
- `mutation = false`, `sanitizer = false`.
- Trusted-repo proof lanes: `focused-tests`, `base-tests-red-green`,
  `actionlint`, `scoped-source-route-checks`.

The tokmd analyze preset is hardcoded to `bun-ub`
(`TOKMD_ANALYZE_PRESET` in src/main.rs); no action input overrides it.
Sensor install uses `tool-bundle: core` (tokmd pinned 1.12.0, cargo-allow,
ripr, unsafe-review, ast-grep, actionlint via
scripts/install-gh-runner-tools.sh). On a generic hosted runner an install
miss is an evidence gap; on the standard image it is drift and should fail
`ub-review doctor --require-core-tools` (docs/GH_RUNNER_BUN.md). The tokmd
sensor now preflights `--version` and names installed vs pinned versions in
the sensor receipt before running `--preset bun-ub` (#319). Cargo-allow now
skips a foreign-dialect `policy/allow.toml` with a linked reason mirrored
through resolved tools, sensor status, tool status, and tool-gate artifacts
(#318); the Bun fork still has no native cargo-allow ledger
(`policy/cargo-allow.toml`), so that sensor remains missing evidence until
maintainers add one.

### Red/green focused proof vs heavy witnesses

Bun v0 sets `allow-heavy: 'false'`. The cheapest-decisive-proof table from
docs/BUN_UB_HUNT.md governs what the hunt expects; what the box actually runs
without a lease is the focused proof lane set above. Heavy witnesses - Miri,
cargo-mutants, ASAN/sanitizers, and leased coverage - are HeavyWitness-class
or leased tools that are skipped with the reason "heavy/manual witness
requires --allow-heavy" unless the workflow grants the lease explicitly
(src/builtin.rs tool registry, src/main.rs proof policy, action.yml
`allow-heavy` input). Coverage is execution-surface telemetry, not proof,
even when leased (docs/BUN_UB_HUNT.md); the coverage sensor also showed a
transient exit-101 failure on a gate run, recovered next run (#313). Proof
broker edge cases (lease `absent` status, `base_patch_failed` lane routing,
manual-cost allowlist) are tracked in #312 and bound how much weight a
red/green receipt can carry at the margins.

### UB-hunt docs and the calibration ledger

docs/BUN_UB_HUNT.md is the operating doctrine: hunt invariant, review-fast PR
shape, proof rules, tool roles, packet reading order, reviewer surface, and
the feedback-loop rule that tool defects get grounded upstream issues
(docs/SENSOR_ROUTING.md) instead of local glue. The xtask precommit receipt
defects (#317, #320, #321) are examples of that loop on this repo's own dev
surface; they do not sit on the Bun action path.

docs/calibration/bun-ub-review-ledger.md is the run-by-run tuning record for
the review compiler and prompts - explicitly not for upstream Bun claims. It
holds the acted-on/dismissed/parked breakdown per run (PR #29 smoke, the PR
#49 pin advance, the PR #28 multi-lane coordination notes, the
allocation-failure false-premise calibration item). This is distinct from the
optional `ledger-path` action input, which injects a read-only UB ledger file
into shared model context (action.yml); Bun v0 passes none.

### Witness artifacts and the ledger relationship

The packet's witness artifacts (`review/witnesses.json`,
`review/witness_registry.json`, `witnesses.ndjson`, schemas
ub-review.witness.v1 / ub-review.witness_registry.v1) record which
evidence - inline comments, findings, observations, proof receipts,
follow-up evidence - backs which reviewer-facing surface. The calibration ledger consumes those packets
after the fact: repeated lane concerns, false premises, and skipped-witness
patterns observed in witness/observation artifacts become prompt and compiler
follow-ups in the ledger (the observation-merge and refuter-demotion rules
both originated there). Packet artifacts are the machine record; the ledger
is the human judgment trail that tunes the next pin.

## Output artifact / user surface

- One grouped Pull Request Review on the Bun PR when reviewer-value content
  survives compilation, with at most 8 inline comments
  (`max-inline-comments: '8'` in the workflow; cap also appears in the
  resource-aware target in docs/ROADMAP.md). Allowed body sections: decision,
  findings, verification questions, proof results, refutations, parked
  follow-ups, specific evidence gaps (docs/BUN_UB_HUNT.md,
  docs/REVIEW_BODY_CONTRACT.md). Banned: lane rosters, provider tables,
  setup/command logs, generic residual risk, no-finding boilerplate,
  missing-tool chatter.
- The full packet under `target/ub-review/`, uploaded by the consumer
  workflow as `ub-review-packet-<pr>` with 7-day retention: `input/`,
  `sensors/`, `lanes/` (one packet per effective lane, `[{lane}]` prefixes),
  `review/` (review.md, review.json, candidates, resolved candidates,
  follow-up artifacts, proof receipts, witnesses, metrics, cache manifest),
  `events.ndjson`, `running-summary.md` (docs/GH_RUNNER_BUN.md).
- Exactly one of `review/github-review.json` (content posted) or
  `review/github-review-skip.json` (artifact-only was correct), never both,
  never neither. Terminal state `sufficient` with a skip receipt is a
  successful gate state, not a failure - that is the artifact-only PR body
  guard the PR #49 pin advance validated.
- `review/post-result.json` / `review/post-error.json` plus the payload and
  stdout/stderr trail for posting diagnostics, exposed as action outputs
  (docs/ACTION_CONSUMER_BUN.md).

## Required fields

Enforced by scripts/verify-bun-review-artifacts.py against the downloaded
packet (defaults already aimed at this preset:
`--expected-review-profile bun-ub-v0`, `--expected-repo-kind bun`):

- the common tree, summary headings, sensor status receipts for all six
  sensors, and the resolved-profile/resolved-plan artifacts;
- lane packets in `lanes/` exactly matching `effective_model_lanes` from the
  resolved plan - extra or missing lane files fail verification;
- `github-review.json` XOR `github-review-skip.json`, with skip status in the
  known set (`skipped_empty_smoke`, `skipped_artifact_only_body`,
  `skipped_pass_policy`, `skipped_gate_failure_artifact_only`; the last status
  is allowed only when a failed gate has no posted review);
- no standalone approval lines (profile sets
  `ban_standalone_approval = true`);
- metrics parity (proof receipts, proof requests, observations, resource
  leases, follow-up task counts);
- secret guards: raw assignments of `FACTORY_API_KEY`, `MINIMAX_API_KEY`,
  `OPENCODE`/`OPENCODE_API_KEY`, GitHub tokens, or known token prefixes
  anywhere in the packet are failures; GitHub secret placeholders
  (`${{ secrets.X }}`), escaped placeholders, and low-diversity synthetic
  values are allowed (SECRET_VALUE_NAMES / SECRET_VALUE_PREFIXES and the
  self-test cases in scripts/verify-bun-review-artifacts.py).

Pin-advance verification additionally requires `--min-ok-model-lanes 10`
(default 0) and `--require-no-model-evidence-failures`
(docs/GH_RUNNER_BUN.md). Note `--min-ok-model-lanes` counts lanes with status
ok or degraded.

## Advisory vs blocking behavior

Everything the Bun PR author sees is advisory. The preset runs
`mode: review-byok`, where `fail-on-gate: auto` resolves to false
(src/cli.rs FailOnGate), the profile configures no tool gate thresholds and
no blocking opt-ins, and the workflow sets `fail-on-post-error: 'false'`. The
Bun PR has no required ub-review check; nothing this preset emits can block a
Bun merge. The enforcement that exists lives one level up: the strict
verifier plus the human pin-advance procedure gate which ub-review SHA the
Bun fork is allowed to run.

All six sensors are advisory evidence. Missing MiniMax key, provider
500s, rate limits, and timeouts degrade lanes to missing model evidence and
never fail the job. Skipped heavy witnesses surface as one plan-level note
("heavy witnesses are disabled unless --allow-heavy is passed") and skipped
sensor receipts; per-lane heavy-skip caveats are filtered as review noise.
Folding them into one global missing-evidence observation is a
calibration-ledger follow-up (docs/calibration/bun-ub-review-ledger.md)
that is not yet implemented.

## Fail-closed behavior

- Missing evidence is recorded as missing evidence, never as clean evidence.
  The smoke run's attempt 1 (MiniMax preflight HTTP 500) producing failed
  model evidence instead of a clean review is the canonical receipt
  (docs/ROADMAP.md).
- The verifier fails closed on packet-shape drift: lane/plan mismatch,
  both-or-neither review payloads, unknown skip statuses, metrics parity
  breaks, standalone approvals, and any raw secret marker.
- The pin discipline fails closed: the Bun gate never floats on `main`; a
  candidate SHA that does not pass both the local verifier and a real Bun
  consumer run does not become the pin (docs/BUN_UB_HUNT.md,
  docs/GH_RUNNER_BUN.md).
- On the standard image, missing core tools are image drift and
  `ub-review doctor --require-core-tools` fails before a Bun packet starts;
  on generic hosted runners the same gap degrades to recorded missing
  evidence (docs/GH_RUNNER_BUN.md). The fail-closed/degrade split is
  deliberate and keyed on `--require-core-tools` / `UB_REVIEW_STANDARD_IMAGE`.

## Trust boundary / non-claims

- The preset does not prove Bun UB-free and never claims to. Model lanes
  investigate; only proof receipts prove, and Bun v0's proof surface is the
  focused red/green set under gh-runner caps (3 focused test files, no
  network, 600s total).
- No Miri, no ASAN, no mutation, no coverage without an explicit
  `allow-heavy` lease; v0 sets `allow-heavy: 'false'`. Source inspection
  alone is not sufficient evidence for any claim class
  (docs/BUN_UB_HUNT.md).
- The verifier proves packet shape and hygiene, not finding truth. A passing
  verifier means the contract held, not that the review was right;
  calibration of rightness lives in the ledger.
- Lane identity and model identity are separate; all v0 lanes happen to be
  MiniMax M3, but packet prefixes use lane names only
  (docs/ARCHITECTURE.md invariant).
- Auxiliary bots (Droid focused review) stay auxiliary and fork-only; their
  output is never called a comparison (docs/ROADMAP.md operating rules).
- Sensor defects are filed upstream with receipts; #318 and #319 are now
  guarded locally by sensor preflight/planning receipts, while the ripr-swarm
  #1052/#1053/#1054 family remains the live example touching this preset's
  sensors.

## Reliance answers

What can a user rely on? The pinned SHA contract: ten MiniMax lanes plus
refuter on SourceUb diffs, one grouped review with at most 8 inline comments,
the full packet uploaded, verifier-checked artifact shape, and
missing-evidence honesty.

What can break the gate? Nothing on the Bun PR. The only hard failures are
the consumer job's own infrastructure (checkout, action resolution, artifact
upload) and, at release time, the verifier blocking a pin advance.

What is only advisory? Everything: all sensor output, all model findings, all
proof results in review-byok mode with no blocking policy.

What is visible in the PR? At most one grouped review per posting pass
(opened, ready_for_review), holding only decision-changing content; quiet
passes post nothing.

What is artifact-only? Lane packets, sensor receipts, provider/preflight
status, metrics, observations, candidates and refuter trail, witnesses, proof
receipts, skip receipts, the running summary.

What does success look like in ten minutes? The `UB evidence packet /
gh-runner` job finishes (the smoke attempt ran about 2m14s), the packet
artifact downloads, `running-summary.md` reads clean, and the verifier passes
with 10 expected lanes - or a `sufficient` terminal state with an
artifact-only skip receipt, which is equally a success.

## Validation commands

```bash
python scripts/verify-bun-review-artifacts.py --self-test
python scripts/verify-bun-review-artifacts.py target/ub-review \
  --min-ok-model-lanes 10 \
  --require-no-model-evidence-failures
ub-review doctor --require-core-tools     # standard image only
cargo test --bin ub-review --locked       # lane routing, refuter, skip receipts
```

Pin-advance procedure: run the Bun consumer workflow on the candidate SHA,
download the packet artifact, run the verifier with the pin-advance flags,
then move the pin in the Bun workflow and record the run in
docs/calibration/bun-ub-review-ledger.md.

## Implementation PR slices

The preset is production; remaining slices harden edges it currently routes
around:

1. DONE (#335, closing #316): the ripr receipt chain is real —
   `sensors/ripr/gate-decision.json` evaluates in production on this repo.
   A Bun-side ripr threshold remains opt-in for the Bun maintainers;
   receipt depth past counts routes through #347.
2. DONE (#318, #319): cargo-allow foreign-dialect ledgers skip with a linked
   reason through tool artifacts, and tokmd version-drift reason is surfaced
   by a run preflight before `--preset bun-ub`.
3. Proof broker edge cases that bound red/green receipt weight: lease
   `absent`, `base_patch_failed` routing, manual-cost allowlist, shell-token
   test gap (#312).
4. Coverage sensor transient-failure hardening before any leased coverage
   story for Bun (#313).
5. DONE: legacy `[gate].synchronize_mode` was deleted with a deprecation
   receipt before recommending `synchronize` triggers to any consumer (#306).
6. Provider remainder before any non-minimax-only Bun policy: future provider
   config choices for model/env/role and prompt-cache wiring.

No slice changes the pinned contract; each lands behind a verifier-checked
pin advance.

## Release note claim

The `bun-ub` preset is production at pin
`EffortlessMetrics/ub-review@804d198b5a15a0df94bb4f43750dba71165916cd`: ten
MiniMax lanes plus a refuter over a deterministic evidence packet, six
sensors, focused red/green proof, one grouped PR review capped at eight
inline comments, and a strict artifact verifier as the release proof. Heavy
witnesses stay off without an explicit lease, artifact-only output is a
successful terminal state, and missing evidence is always reported as
missing evidence.
