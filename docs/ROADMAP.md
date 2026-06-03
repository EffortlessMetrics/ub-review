# Roadmap

`ub-review` is the Bun UB review gate first. General review-engine work should
come after the Bun profile is proven and should preserve the Bun invariants.

## Current locked baseline

The v0 gate is:

- `EffortlessMetrics/ub-review@v0`;
- `review-direct`;
- MiniMax-only, 10 Bun lanes plus refuter;
- one grouped Pull Request Review;
- at most 8 inline comments;
- full artifact packet;
- missing evidence reported as missing evidence, never as clean evidence.

The fork-only Bun smoke proof is PR `EffortlessSteven/bun#29`.

Verified v0 smoke evidence:

- workflow: `UB Review Packet`;
- run: `26843669614`, attempt 2;
- artifact: `ub-review-packet-29`;
- artifact digest:
  `sha256:2ec165d6487ef1f9c78999e0cf25ce6be4ead78762c68a8acaf4f5b3a9c6ac24`;
- strict verifier passed with 10 expected MiniMax lanes;
- inline comments: 3;
- off-diff comments: 0;
- missing sensor evidence: 0;
- missing model evidence: 0;
- GitHub review post HTTP status: 200.

Attempt 1 on the same run hit a MiniMax preflight HTTP 500 and correctly
rendered missing model evidence instead of a clean review. That behavior is part
of the release contract.

Auxiliary Droid focused review remains optional and fork-only:

- workflow: `Droid Focused Review`;
- lanes: `tests` and `ub`;
- model: MiniMax M3;
- proof PR: `EffortlessSteven/bun#30`;
- both focused lanes passed on the MiniMax configuration;
- GLM/Z.AI is not used by the fork Droid workflow.

## Operating rules

- Work PR by PR.
- Keep `ub-review` source-of-truth in `EffortlessMetrics/ub-review`.
- Keep Bun workflow changes on `EffortlessSteven/bun:main`.
- Do not push review tooling into upstream `oven-sh/bun` branches.
- Keep UB fixes in isolated fork branches.
- Keep Droid, when enabled, as auxiliary focused review only.
- Do not call auxiliary review output a comparison in workflow names or PR text.
- File real `tokmd`, `ripr`, or `unsafe-review` defects in the matching
  `*-swarm` repo with repro evidence instead of silently forking behavior here.

## Resource-aware orchestrator target

The next architecture is a resource-aware review orchestrator. It should use the
box fully without letting lanes duplicate tests, exhaust disk, or post directly.

```text
evidence store      = shared packet, sensor receipts, diff map, ledger context
lane workers        = specialist model questions
orchestrator        = dispatcher, context router, editor, referee
proof broker        = central test/tool scheduler
resource broker     = CPU/memory/disk/time governor
review compiler     = only component allowed to post
```

The orchestrator is not another free-roaming reviewer. Lanes investigate and
emit structured events. The orchestrator routes evidence, asks bounded follow-up
questions, collects proof requests, and sends allowed commands to the proof
broker. The compiler validates, dedupes, caps, and posts one grouped review.

The first missing layer is shared intra-run memory. Direct provider lanes cannot
read files while a model call is running, so the runner owns observation reads
and writes. The runner injects relevant existing observations into prompts and
collects new observations from model output.

Standard mode should optimize for review quality under a hard runtime budget, not
minimum wall time. The target is 8-10 minutes with a 15-minute hard cap on
GitHub-hosted runners. That extra time should buy coordination and confirmation:

```text
model lanes: 10
follow-up budget: 4-6 calls
focused proof budget: 1-2 commands
inline comments: max 8
```

Standard mode should keep the current speed shape:

```text
pass 1: 10 candidate lanes in parallel
pass 2: observation-aware refuter/finalizer
pass 3: review compiler
```

The first pass should start read-only lanes immediately against the stable
packet:

```text
diff map
changed files
shared_context.md
sensor status
available cached receipts
lane prompt
```

Do not block every lane on every sensor, proof command, or follow-up. Direct
provider lanes cannot read `observations/*.ndjson` while a model call is already
running, so the runner owns the shared-memory turn:

```text
phase 1: read-only lanes start immediately
phase 2: runner reads observations, proof receipts, sensor deltas, and targeted
         excerpts
phase 3: runner injects relevant deltas into bounded follow-up prompts
phase 4: refuter/compiler produces one review
```

This keeps the fast first pass while letting the second turn reduce repetition,
refute false premises, and coordinate proof requests. Deep mode can later add
targeted confirmation questions or staggered waves. Do not replace direct model
fanout with agent-style filesystem access.

The core control loop should become:

```text
1. Resolve profile and runtime budget.
2. Build diff and RIGHT-side line map.
3. Run deterministic sensors.
4. Build evidence store.
5. Start first-pass lane questions.
6. Watch lane events and candidate findings.
7. Route targeted evidence to follow-up questions.
8. Dedupe proof requests.
9. Run allowed central proof jobs once.
10. Route proof receipts back to lanes and compiler.
11. Confirm, refute, or demote candidates.
12. Compile one grouped Pull Request Review.
13. Write metrics, calibration, and artifact receipts.
```

Only the proof broker runs local commands. Only the review compiler posts. The
resource broker owns leases for CPU, memory, disk, timeout, network, and scratch
space.

Example lane output:

```json
{
  "lane": "tests-oracle",
  "question": "red-green",
  "status": "needs-proof",
  "claim": "The added test may not prove red/green behavior.",
  "evidence_needed": ["focused test on HEAD", "optional old-main witness"],
  "priority": "high",
  "cost_hint": "focused-test"
}
```

Example observation artifact:

```text
target/ub-review/observations/
  ub-active-view.ndjson
  tests-red-green.ndjson
  source-route.ndjson
  ...
target/ub-review/review/observations.json
target/ub-review/review/unique_observations.json
target/ub-review/review/merged_observations.json
target/ub-review/review/dropped_observations.json
```

Observation schema:

```json
{
  "schema": "ub-review.observation.v1",
  "id": "obs-tests-oracle-0001",
  "lane": "tests-oracle",
  "question": "red-green-proof",
  "claim": "The new tests need a witnessed red/green run against old main.",
  "kind": "missing-evidence",
  "status": "open",
  "severity": "medium",
  "confidence": "high",
  "path": "test/js/bun/md/md-edge-cases.test.ts",
  "line": 1145,
  "fingerprint": "sha256:...",
  "evidence": ["test sensor skipped", "PR body claims old code SEGV"],
  "dedupe_key": "markdown-rab-red-green-witness"
}
```

Observation kinds:

```text
bug
verification-question
missing-evidence
test-gap
source-route-gap
security-risk
false-premise
parked-follow-up
resolved-check
```

Observation statuses:

```text
open
covered
confirmed
refuted
demoted
parked
duplicate
```

The compiler should dedupe observations deterministically first:

```text
dedupe_key
else normalized(path + line + claim)
else model-assisted merge for leftovers
```

Normalize repeated global caveats such as skipped tests/builds/Miri, repeated
SAB negative-test suggestions, resizable-flag propagation questions, and
missing actionlint/tool evidence. The final summary should describe each unique
observation once, with contributing lanes listed where useful.

Example proof receipt path:

```text
proof/<proof-id>/
  receipt.json
  stdout.txt
  stderr.txt
  summary.md
```

This layer is how `ub-review` should eventually answer questions like:

- two lanes requested the same focused test, so it ran once;
- a false premise was routed to confirmation before becoming inline;
- a test was skipped because the runtime profile had no budget;
- missing proof was recorded as missing evidence, not safety.

## Repo-style ledger correction

For Rust repositories that adopt this style, avoid hand-rolling separate TOML
ledgers for every source exception class. Prefer `cargo-allow` as the single
source-tree exception ledger and keep `xtask` as the orchestrator for
repo-specific gates, release readiness, CI planning, and evidence verification.
The durable model is documented in
[SOURCE_EXCEPTION_LEDGER.md](SOURCE_EXCEPTION_LEDGER.md).

```text
cargo-allow owns:
  policy/allow.toml
  source exception inventory
  exception ownership
  evidence links
  review_after / expires
  PR diff summaries
  agent worklists

xtask owns:
  orchestration
  repo-specific gates
  CI planning / LEM
  release readiness
  calling cargo-allow
  aggregating receipts
```

Keep separate semantic evidence tools in place. `cargo-allow` says that a
visible source exception is owned and evidenced; Clippy/rustc, `ripr`, mutation,
coverage, `unsafe-review`, `cargo-deny`, and `cargo-vet` say whether the cited
evidence is real enough for the gate.

## Next PRs

### 1. Smoke cleanup

Close stale smoke PRs and remove local smoke worktrees after their evidence is
captured. Do not delete remote branches unless that cleanup is explicitly
requested.

Acceptance:

- the final clean smoke proof remains traceable in this roadmap;
- old smoke PRs no longer clutter the fork queue;
- no upstream Bun branch was touched.

### 2. Review efficiency metrics

Add factual runtime and review-efficiency fields to artifacts and summaries:

- wall-clock seconds;
- model lane counts;
- provider failures;
- inline comments posted;
- off-diff candidates rejected;
- summary body bytes;
- post status.

Acceptance:

- `review/metrics.json` records the efficiency facts;
- `running-summary.md` has a short review-efficiency block;
- no quality score is invented.

### 3. Clean PR review body rendering

Lane rosters are setup metadata, not reviewer value. A successful
provider/model/lane table belongs in artifacts, not in the GitHub PR Review
body.

Add separate renderers or policy paths for:

```text
render_review_body_for_pr(...)
render_review_body_for_artifact(...)
render_running_summary(...)
```

The PR review body should answer reviewer questions:

```md
## Decision

## Confirmed findings

## Verification questions

## Summary-only concerns

## Refuted / dropped

## Residual risk

## Missing evidence
```

Do not include a successful model-lane status table in the PR review body.
Successful lane/provider/model status is artifact metadata.

Only mention execution status in the PR body when it changes trust:

- a model lane failed;
- a provider failed;
- a sensor failed or was skipped and affects confidence;
- a proof request could not run;
- the review is partial or degraded.

Add review-body policy:

```toml
[review_body]
include_successful_lane_table = false
include_provider_table = "on_failure"
include_sensor_table = "on_failure"
include_execution_summary = "none"
```

For Bun v0, keep:

```toml
[review_body]
include_successful_lane_table = false
include_provider_table = "on_failure"
include_sensor_table = "on_failure"
include_execution_summary = "none"
```

Acceptance:

- a fully successful 10-lane review body does not contain `## Model lanes`;
- a fully successful review body does not list every provider/model/lane status;
- failed provider/model/sensor/proof evidence still appears under missing or
  failed evidence;
- `review.json`, `metrics.json`, `running-summary.md`, `lanes/*.md`, and future
  observations keep the full audit trail;
- tests cover both successful and degraded review-body rendering.

### 4. Calibration ledger

Track acted-on findings, dismissed false premises, parked follow-ups, and prompt
or compiler changes.

Acceptance:

- `docs/calibration/bun-ub-review-ledger.md` records PR #29 and later real UB
  runs;
- known false premises are linked to prompt/compiler follow-up;
- calibration entries do not become product claims.

### 5. Observation ledger artifacts

Add the append-only observation surface:

```text
target/ub-review/observations/
target/ub-review/review/observations.json
```

Acceptance:

- lanes can emit observations;
- observations are artifacted as NDJSON;
- observations include `kind`, `status`, `confidence`, `dedupe_key`, and
  evidence fields;
- no posting behavior changes yet.

Initial implementation may derive observations from the current compiler outputs:
validated inline comments, summary-only findings, and missing evidence. Dedicated
model-emitted observations belong to the lane output split.

### 6. Lane output split

Change lane output from findings-only to:

```json
{
  "observations": [],
  "candidate_findings": [],
  "failed_objections": [],
  "proof_requests": []
}
```

Acceptance:

- fixtures are migrated or backward-compatible;
- tests prove observations render into artifacts;
- existing candidate finding behavior is preserved.

### 7. Summary dedupe from observations

Teach the compiler to merge repeated observations before writing `review.md`.

Acceptance:

- the same claim from multiple lanes appears once;
- contributing lanes are recorded;
- repeated missing test/build/Miri evidence appears once globally;
- `unique_observations.json`, `merged_observations.json`, and
  `dropped_observations.json` are written.

### 8. False-premise refuter calibration

Add calibration for the known allocation-failure premise:

```text
Box::from(slice) cannot return None.
Allocation failure is not a recoverable fallback.
The None path means the guard did not match.
```

Acceptance:

- PR #28-style allocation-failure candidates are dropped or summary-only
  refuted;
- this class is not posted inline;
- the calibration ledger records the rule.

### 9. Proof request skeleton

Add proof request artifacts without running commands yet:

```text
proof_requests.ndjson
review/proof_request_groups.json
proof_plan.md
```

Acceptance:

- lanes can request focused tests;
- duplicated proof requests are grouped;
- compiler lists requested proof once;
- disabled proof is explicit missing evidence.

### 10. Profile extraction

Move proven Bun defaults into data:

```text
profiles/bun-ub-v0.toml
```

A profile is:

```text
tools + lanes + providers + budgets + posting + guards + artifacts
```

Acceptance:

- Bun behavior is unchanged;
- emitted `resolved-profile.json` matches current v0 behavior;
- emitted `resolved-plan.json` explains exact tools, lanes, budgets, posting
  policy, and guards.

### 11. Selectors

Add composition without weakening Bun:

```text
--lanes
--except-lanes
--tools
--except-tools
--depth quick|standard|deep
```

Acceptance:

- users can run a subset of lanes or sensors;
- selectors are recorded in `resolved-plan.json`;
- Bun v0 defaults remain sharp.

### 12. Runtime profiles

Add runtime profiles separately from review profiles:

```text
gh-runner
cx23
cx33
cx43
local-dev
```

Runtime profiles control:

- model concurrency;
- local tool concurrency;
- max focused tests;
- max test seconds;
- scratch and artifact budgets;
- whether builds, Miri, mutation, or other heavy witnesses are allowed.

Acceptance:

- runtime profiles control cost and concurrency;
- Bun v0 defaults remain sharp.

### 13. Event and blackboard model

Expand the observation ledger into stable structured artifacts for lane and
orchestrator communication:

```text
events.ndjson
candidates/<candidate-id>.json
proof_requests/<proof-id>.json
questions/<lane>/<question>.json
```

Acceptance:

- every lane/question writes structured results;
- the review compiler reads candidates from structured artifacts;
- events are append-only;
- missing, failed, skipped, and timed-out states are explicit.

### 14. Question graph

Add question-level lane configuration:

```text
lanes[].questions[]
```

Acceptance:

- one lane can run multiple bounded questions;
- each question declares the evidence slices it receives;
- question results are artifacted separately;
- the 10-lane Bun behavior can be represented without changing output.

### 15. Orchestrator skeleton

Start deterministic. The first orchestrator does not run shell commands and does
not post. It reads candidates, groups them by evidence need, routes available
evidence, and creates follow-up question tasks.

Acceptance:

- duplicate evidence needs are grouped;
- PR #28-style false-premise candidates can be routed to confirmation before
  posting;
- uncertain follow-ups are demoted to summary-only or dropped;
- the compiler still enforces all posting guardrails.

### 16. Proof broker v0

Add central proof requests and receipts.

For v1, allow only focused tests behind profile policy:

- command allowlist;
- timeout;
- stdout/stderr capture;
- exit status;
- dedupe identical commands;
- no source edits.

Acceptance:

- two lanes requesting the same focused command produce one proof run;
- all lanes consume the same receipt;
- skipped proof is missing evidence;
- proof jobs cannot bypass runtime budget.

### 17. Resource broker

Add leases for local work:

```text
cpu
memory
disk
timeout
network
scratch
```

Acceptance:

- focused tests and local tools cannot exceed profile budget;
- unavailable resources queue, skip, or demote proof requests explicitly;
- scratch cleanup is guaranteed;
- heavy witnesses remain disabled by default on `gh-runner`.

### 18. Confirmation and refutation pass

Add a bounded follow-up pass for top candidates.

Candidate dispositions:

```text
inline
summary-only
parked-follow-up
refuted
dropped
```

Acceptance:

- false premises are dropped or refuted before inline posting;
- plausible but unproven concerns become summary-only;
- inline comments still require valid RIGHT-side diff lines;
- the no-LGTM and missing-evidence rules still hold.

### 19. First real UB run report

After the first non-smoke Bun UB PR uses the gate, record:

- runtime;
- acted-on findings;
- dismissed findings;
- parked follow-ups;
- what changed because of the review;
- whether auxiliary Droid focused lanes earned their keep.

Acceptance:

- report is factual and evidence-backed;
- it identifies tuning work without broadening the product.

### 20. Sensor image and base cache

Make the standard runner image supply the core evidence tools before review
time.

Acceptance:

- `tokmd`, `ripr`, `unsafe-review`, and `ast-grep` are installed on the
  standard image and visible on `PATH`;
- `tokmd` is versioned for the Bun profile and emits on-diff `analyze`,
  compact `cockpit`, and bounded changed-file `context` receipts;
- `tokmd analyze --preset bun-ub` replaces the current verified effort-delta
  preset after `tokmd` exposes that preset
  (`EffortlessMetrics/tokmd-swarm#182`);
- `ub-review doctor --require-core-tools` fails image drift early;
- `ub-review cache warm` writes a base-tree manifest keyed by base tree SHA,
  profile hash, and tool versions;
- GitHub-hosted fallback still records missing evidence instead of claiming a
  clean review;
- no fake heavyweight index is created for tools that do not expose one.

### 21. Release binary path

Move the action from source build by default to release binary download with
source build fallback.

Acceptance:

- action startup is materially faster;
- no correctness depends on a durable cache;
- source fallback still works.

### 22. Deep mode

Add more candidate/refuter pressure without increasing PR noise.

Acceptance:

- deep mode can run more investigations;
- max inline comments remains capped;
- candidate-only lanes cannot directly post high-severity comments;
- provider failures remain evidence, not findings.

## Later profiles

Generalize beside Bun, not by weakening Bun:

```text
profiles/rust-unsafe.toml
profiles/rust-test-proof.toml
profiles/js-native-boundary.toml
profiles/github-action-security.toml
```

Each profile should choose its own tools, lanes, providers, budgets, posting
policy, and guards.
