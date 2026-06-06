# UB-REVIEW-SPEC-0005 - sensor and tool integration surface

Status: authored 2026-06-06 (release surface spec wave, docs-only).
Umbrella: [UB-REVIEW-SPEC-0001](UB-REVIEW-SPEC-0001-release-surface.md).
Maturity: partial. The tool registry, trigger scoping, sensor receipts, and
packet routing are production - they run on every gate pass of this repository
and behind the Bun pin. Two gaps are named, not papered over: the
`[tools.*.gate]` threshold mechanism is implemented and unit-tested but has
evaluated on zero production runs (#316), and the standard-image version pin
covers only tokmd - ripr and unsafe-review drift undetected (part of #316).

## Purpose

Define how the instruments - `tokmd`, `cargo-allow`, `ripr`, `unsafe-review`,
`ast-grep`, `actionlint`, and the leased `coverage` witness - feed evidence
into the gate: the `[tools.*]` registry that declares them, the trigger
taxonomy that scopes them to the diff, the receipt files they must emit, the
packet policy that routes their output to model lanes, and the exact paths by
which a sensor result can (and cannot) change the gate verdict. The boundary
from umbrella 0001 holds everywhere: instruments emit artifacts; ub-review
decides how artifacts affect review and gate behavior; sensor defects are
fixed upstream, never silently absorbed.

## User question

```text
How do tokmd, ripr, unsafe-review, cargo-allow, ast-grep, actionlint,
and coverage feed the gate?
```

Short answer: through receipts, on three distinct paths. (1) A sensor marked
`required = true` whose trigger matched must produce evidence, or the gap
blocks in `intelligent-ci` mode. (2) A sensor with an opt-in
`[tools.<id>.gate]` threshold blocks when its `gate-decision.json` receipt
exceeds the threshold. (3) Everything else is routed context for model lanes
and artifacts for audit - advisory, never blocking. Successful tool status
never reaches the verdict or the PR body.

## Lifecycle moment

Every `plan`/`run` pass. `prepare_plan` classifies the diff and resolves each
registered tool's trigger into a per-sensor plan (`build_plan`,
src/main.rs); fast sensors run in the first evidence window
(docs/ci/work-queue.md packet timing: sensors and must-run static checks in
roughly T+0-60s); the initial model packet closes with completed receipts
plus the pending queue; tool gate outcomes are evaluated when tool artifacts
are written; the gate outcome folds required-sensor gaps and tool-gate
failures at review compile time. `doctor` is the pre-run moment: it reports
tool presence and version per registered tool and can hard-require the core
set.

## Consumer

```text
model lanes        lanes/<lane>.md "Routed sensor evidence" - per-sensor
                   status from the lane's receives list (src/main.rs
                   write_lane_packets)
the gate           tool-gate-outcomes.json entries and required-sensor
                   evidence gaps folded into review/gate_outcome.json
the verifier       all six core sensor status receipts required; tool-status
                   must mirror resolved-tools or verification fails
                   (scripts/verify-bun-review-artifacts.py;
                   docs/ci/work-queue.md)
the operator       doctor output: found/missing/version/rule-cache per tool
upstream tools     defect reports with receipts (see trust boundary)
```

The six core sensors are a named constant: `CORE_REVIEW_TOOLS = [tokmd,
cargo-allow, ripr, unsafe-review, ast-grep, actionlint]` (src/main.rs).
Coverage is not core; it is a leased heavy witness this repository chooses to
lease on every PR.

## Inputs

### The tool registry

Every instrument is a `[tools.<id>]` entry. The accepted keys are pinned as
`KNOWN_TOOL_POLICY_KEYS` (src/config.rs):

```text
id  command  class  weight  default  required
timeout_sec  artifact_budget_mb  requires_lease  enabled  gate
```

An inline test pins this list to the `ToolPolicy` struct's serialized field
set so the sanitizer's unknown-key receipts can never drift from the code
(src/main.rs inline tests). Unknown or invalid keys are stripped per-key with
`PolicyError` receipts - a policy the repo wrote is never silently replaced
(src/config.rs `sanitize_policy_sections`; same contract as spec 0003).

Key semantics:

- `class` - one of `packet`, `static` (default), `search`, `workflow`,
  `security`, `coverage`, `test`, `build`, `heavy-witness` (src/config.rs
  `ToolClass`). Class interacts with runtime profile limits: `test`/`build`
  classes are skipped when the profile grants zero test/build leases
  (src/main.rs plan construction).
- `default` - the trigger. Taxonomy (src/config.rs `Trigger`, kebab-case):
  `always`, `source-changed`, `source-exception-changed`,
  `rust-behavior-or-tests-changed`, `unsafe-or-native-risk-changed`,
  `workflow-changed`, `dependency-changed`, `shell-changed`, `cpp-changed`,
  `diff`, `manual`, `never` (the default). Triggers resolve against
  classified diff flags - e.g. `rust-behavior-or-tests-changed` fires when
  `flags.rust_changed || flags.rust_tests_changed` (src/main.rs trigger
  match). A docs-only diff legitimately runs fewer sensors.
- `required` - opts the sensor into gate-blocking evidence semantics
  (intelligent-ci mode only; see advisory vs blocking).
- `timeout_sec` / `artifact_budget_mb` - per-sensor execution budget and
  artifact budget. `timeout_sec` is recorded verbatim into all three of
  `resolved-tools.json`, `tool-status.json`, and the sensor status receipt;
  `artifact_budget_mb` is recorded into `resolved-tools.json` and
  `tool-status.json` only.
- `requires_lease` - marks a heavy witness. Without `--allow-heavy` the plan
  skips it with the receipted reason `heavy/manual witness requires
  --allow-heavy` (src/main.rs plan construction).
- `enabled` - registry membership without execution; `enabled = false`
  produces a skipped receipt (`disabled by config`), not silence.
- `gate` - the opt-in `[tools.<id>.gate]` threshold sub-table: `scope`
  (`"on-diff"` is the only scope with semantics; other values are stripped
  with a receipt) and `max_new_unsuppressed` (src/config.rs
  `ToolGatePolicy`).

### Builtin defaults

The binary ships 25 builtin tools (src/builtin.rs). The six core sensors as
shipped:

```text
tokmd          packet    always                          180s  128MB
cargo-allow    static    source-exception-changed        120s   64MB
ast-grep       search    source-changed                   60s   64MB
ripr           static    rust-behavior-or-tests-changed  240s  128MB
unsafe-review  static    unsafe-or-native-risk-changed   240s  128MB
actionlint     workflow  workflow-changed                 60s   32MB
```

Coverage as shipped: class `coverage`, trigger `manual`,
`requires_lease = true`, 1800s/256MB, disabled by default (src/builtin.rs).
Heavy witnesses `miri` and `cargo-mutants` are `heavy-witness` class, weight
99, manual trigger, leased. Repo config overrides builtins per key: this
repository's `.ub-review.toml` makes the five non-tokmd core sensors
`required = true`, keeps tokmd `required = false`, and enables coverage with
`default = "always"`, `required = false`, `requires_lease = true`.

### Run-environment inputs

- `doctor --require-core-tools` (or `UB_REVIEW_STANDARD_IMAGE`): doctor
  bails when a core tool is missing or a pinned version mismatches
  (src/main.rs `cmd_doctor`). The pin table is honest and thin:
  `expected_standard_image_tool_version` returns `1.12.0` for tokmd and
  `None` for everything else (src/main.rs `STANDARD_IMAGE_TOKMD_VERSION`).
  ripr and unsafe-review version drift is currently undetectable by doctor -
  named as part of #316.
- The action's sensor install step (scripts/install-gh-runner-tools.sh) pins
  tokmd (default 1.12.0, `UB_REVIEW_TOKMD_VERSION`) and actionlint
  (v1.7.12, `UB_REVIEW_ACTIONLINT_VERSION`) but installs cargo-allow, ripr,
  and unsafe-review unpinned - the install-time half of the same drift gap.
- `--allow-heavy` (plan/run flag; `allow-heavy` action input) leases the
  heavy classes. This repository's gate workflow sets `allow-heavy: 'true'`
  and installs `cargo-llvm-cov` so the coverage sensor runs on every PR
  (.github/workflows/ub-review-gate.yml).

## Output artifact / user surface

```text
sensors/<id>/ub-review-sensor-status.json   per-sensor status receipt
sensors/<id>/stdout.txt, stderr.txt         raw command output receipts
sensors/<id>/<tool-specific outputs>        see below
sensors/<tool>/gate-decision.json           threshold input (where produced)
resolved-tools.json                         the resolved registry
tool-status.json                            registry + execution status
tool-gate-outcomes.json
  (+ tool_gate_outcomes.ndjson)             threshold evaluations
work_queue.json / work_events.ndjson        sensor tasks + packet status
lanes/<lane>.md                             routed sensor evidence section
running-summary.md                          missing-evidence section
```

Tool-specific outputs are declared per sensor (src/main.rs
`sensor_outputs`): tokmd writes `commands.json`, `analyze.md/.json`,
`cockpit.md/.json`, `context.md`; cargo-allow writes `cargo-allow.md` and
`cargo-allow.receipt.json`; ast-grep (and semgrep/gitleaks when enabled)
write `report.json`; coverage writes `status.json`,
`coverage-summary.json`, `changed-lines.json`, `upload.json`, `lcov.info`.
Every sensor gets `stdout.txt`/`stderr.txt`; tokmd's subcommands get
per-subcommand stderr receipts (src/main.rs tokmd sensor runner).

`resolved-tools.json` and `tool-status.json` exist at the out root and are
mirrored under `review/` for packet posterity; `tool-status.json` must
mirror the stable tool metadata from `resolved-tools.json` (timeout, budget,
lease flag, gate policy, artifact paths) - the artifact verifier rejects
drift because the queue cannot be audited if status receipts describe a
different tool plan (docs/ci/work-queue.md).

### Packet policy: how sensor output reaches model lanes

Sensor queue tasks are generated from the registry. Each gets a packet
policy (src/main.rs `work_queue_sensor_packet_policy`): required sensors are
`must-run`, planned non-required sensors are `include-if-ready`, unplanned
ones are `artifact-only`. Priority follows the same split
(high/medium/low), and the queue gate policy string is `gate-required` for
required sensors, `trust-affecting` for gate-threshold tools,
`review-context` for other planned sensors, `artifact-only` otherwise
(src/main.rs `work_queue_sensor_gate_policy`).

`initial_packet_status` records what the first model packet can know
(docs/ci/work-queue.md; src/main.rs `work_queue_initial_packet_status`):
`ready_for_initial_packet` when the receipt exists at queue-write time,
`pending_initial_packet` when planned but not yet receipted,
`not_initial_packet` when skipped or artifact-only. The contract sentence
that matters: **late is not missing** - a task that misses the packet
deadline becomes pending queue state and may still produce a receipt for
late-follow-up routing. Lanes are told which concerns may be answered later
so they do not treat unfinished proof as permanent missing evidence.

Receipts route only where they can change output (src/main.rs
`work_queue_sensor_consumers`; docs/ci/work-queue.md): ripr to
tests-oracle/proof-planner/compiler; coverage to
tests-oracle/source-route/compiler; unsafe-review to
ub-memory-lifetime/security/compiler; actionlint to the workflow lanes;
ast-grep to source-route; cargo-allow to security; tokmd to all lanes. Each
lane packet renders a "Routed sensor evidence" section listing each routed
sensor's receipt status (`receipt-absent` when none exists) and instructs
the model: "Do not infer safety from missing sensor receipts."
(src/main.rs `write_lane_packets`).

## Required fields

`sensors/<id>/ub-review-sensor-status.json` (src/main.rs
`write_sensor_status`):

```text
sensor          tool id
status          ok | failed | skipped | missing | timed_out
command         the exact argv executed (display form)
duration_ms     execution time
reason          why, for every non-ok status
outputs         declared artifact file list for this sensor
exit_code       nullable
timed_out       bool (mirrors the timed_out status; a timeout sets
                status = timed_out, never a quiet skip)
timeout_sec, class, requires_lease, required
gate            present only when a [tools.<id>.gate] policy is configured
```

`tool-gate-outcomes.json` entries (src/main.rs `ToolGateOutcomeEntry`):
tool id, configured policy, `planned_run`, `sensor_status`/`sensor_reason`,
`sensor_receipt_path` (`sensors/<id>/ub-review-sensor-status.json`),
`status_source: "tool-status.json"`, `outcome` (`passed` | `failed` |
`missing_evidence` | `not_evaluated`), `evaluated`, `reason`,
`metrics.new_unsuppressed`, `source_artifacts`, and the hardcoded markers
`packet_policy: "gate-only"`, `gate_policy: "trust-affecting"`.

`sensors/<tool>/gate-decision.json` is the threshold input the sensor side
must produce; the evaluator reads a `new_unsuppressed` count from it
(src/main.rs `ToolGateDecision`, `evaluate_tool_gate_threshold`). No core
sensor produces this receipt today - see #316.

Coverage's own receipts state their epistemics in-band:
`changed-lines.json` ships `status: "not_collected"` ("changed-line
coverage is not computed by the local coverage sensor yet") and both it and
`upload.json` carry `execution_surface_only: true` and
`correctness_claim: false` (src/main.rs `write_coverage_status_receipt`).
The work-queue routing table names a changed-line coverage receipt route;
the receipt honestly says the data is not collected yet.

## Advisory vs blocking behavior

Three paths from sensor to gate, all receipted; everything else is advisory:

1. **Required sensor evidence** (`required = true`, trigger matched): the
   evidence-issue collector flags any status other than `ok` -
   `receipt-absent`, `failed`, `missing`, `timed-out`, and skips that should
   have run (src/main.rs `collect_sensor_evidence_issues`,
   `is_sensor_evidence_issue`). These gaps block **only in `intelligent-ci`
   mode**; `review-byok` records the same gaps as
   `evidence_gaps_advisory` (spec 0003; src/main.rs gate outcome
   construction).
2. **Tool gate thresholds** (`[tools.<id>.gate]`, opt-in): only tools with a
   configured gate entry produce outcomes - a tool without one cannot redden
   the gate no matter how it fails. Only the `failed` outcome blocks
   unconditionally. `missing_evidence` blocks only when the tool is required
   AND the repo opted into `[gate.blocking].tool_gate_missing_evidence`
   (default false). `not_evaluated` covers unplanned runs and policies with
   no supported threshold.
3. **Sensor commands as required proof**: this repository folds
   cargo-fmt/check/test/clippy/doc and the artifact verifier into the gate
   as `required = true` sensors plus `[[proof.required]]` entries
   (.ub-review.toml) - that path is spec 0003's contract, not a special
   sensor power.

Never blocking: successful tool status, optional sensor gaps, sensor stdout
volume, advisory findings, coverage percentages. Live receipt for the
coverage case: on PR #305's red run the coverage sensor failed transiently
(exit 101) and stayed advisory (`evidence_gaps_advisory: 1`; coverage is
`required = false` here), recovering on the next run without intervention
(#313).

Known gap #316, stated plainly: `[tools.ripr.gate] max_new_unsuppressed = 0`
is configured on this repository, but the threshold has evaluated on zero
production runs. The ripr invocation emits start-here advisory text, nothing
writes `sensors/ripr/gate-decision.json`, every run lands on
`missing_evidence`, and with the blocking opt-in defaulting to false the gap
stays advisory. The threshold mechanism is unit-tested; the ripr receipt
chain is not yet real. The gate is silently weaker than its policy text says
until #316 lands - and even the receipt ripr 0.5.0 can emit today would not
parse (no `new_unsuppressed` field).

PR-visible: nothing, by default. Sensor tables post only `on_failure` under
this repository's `[review_body]` policy and lane rosters never post
(docs/REVIEW_BODY_CONTRACT.md). The gate check and the artifacts carry the
sensor story.

## Fail-closed behavior

- A missing tool is an evidence gap, never clean evidence: command not on
  PATH writes a `missing` status receipt with the attempted argv
  (src/main.rs sensor execution); an absent receipt is surfaced as
  `receipt-absent`; both are evidence issues for required sensors. On a
  generic runner a missing optional tool degrades the review, not the
  verdict; under `doctor --require-core-tools` (the standard-image posture)
  a missing core tool is a hard failure before the run starts.
- A malformed `gate-decision.json` on an `ok` sensor is `missing_evidence`
  with the parse failure named in the reason - never `passed` (src/main.rs
  tool gate outcome construction).
- A sensor whose status is not `ok` cannot have its threshold evaluated;
  the outcome says so explicitly ("the sensor did not produce a verdict").
- Unknown `[tools.<id>]` keys and unsupported `gate.scope` values are
  stripped per-key with `PolicyError` receipts that become `kind = "policy"`
  blocking reasons - the same parse-error fail-closed as spec 0003. Valid
  sibling keys survive (src/config.rs).
- Timeouts are receipted (`timed_out: true`) and classified as evidence
  issues, not as quiet absence.

## Trust boundary / non-claims

```text
Instruments emit artifacts.
ub-review decides how artifacts affect review and gate behavior.
Sensor defects are fixed upstream, never silently absorbed.
Missing evidence is recorded as missing evidence, never as clean evidence.
```

Non-claims: no sensor result proves code correct or UB-free; ripr is static
mutation-exposure signal, unsafe-review is reviewability signal, neither is
a runtime witness (docs/REPO_OPERATING_HANDOFF.md). Coverage is
execution-surface telemetry, never a correctness proof - the receipts carry
`correctness_claim: false` in-band, the porting baseline spells out both
directions ("missing coverage: unknown execution-surface telemetry, not
test failure; high coverage: execution signal, not correctness proof",
docs/PORTING_BASELINE.md), and the gate workflow uploads lcov to Codecov
with `fail_ci_if_error: false` because a codecov.io outage must not block
merges when ub-review/gate is the only required check
(.github/workflows/ub-review-gate.yml).

The upstream-first discipline has a live, receipted example: the 2026-06
sensor dogfood pass filed defects against the instruments instead of
patching local glue - ripr-swarm#1035-#1038, unsafe-review-swarm#1516-#1518,
cargo-allow#1467-#1470, and tokmd-swarm#219-#221 - with the local
glue-visibility issues tracked here as #317 (xtask precommit buffers
unbounded sensor stdout into receipt markdown; 450 MB ripr.md on a 26 KB
diff), #318 (cargo-allow red-fails on a foreign-dialect ledger instead of
skipping with a linked reason), #319 (tokmd below-pin rejection should name
the version mismatch), #320 (precommit records missing tools as
`success: true` skips, indistinguishable from relevance skips), and #321
(missing-tool receipts should say how to install the tool). Local
workarounds must link the upstream issue (docs/ARCHITECTURE.md sensor-defect
rule); none of these were absorbed silently.

## Validation commands

```bash
cargo test --bin ub-review --locked
                       # pins KNOWN_TOOL_POLICY_KEYS to the struct field
                       # set; tool-gate outcome, sensor-evidence-issue, and
                       # policy sanitizer contracts live in the inline tests
ub-review doctor --require-core-tools
                       # core six present, tokmd pinned at 1.12.0; bails on
                       # missing tools or version mismatch
ub-review plan --write --out target/ub-review
                       # resolved-tools.json with per-sensor trigger
                       # decisions and skip reasons, no execution
python scripts/verify-bun-review-artifacts.py target/ub-review \
  --expected-review-profile ub-review-self --expected-repo-kind ub-review
                       # all six sensor status receipts required;
                       # tool-status/resolved-tools mirror enforced
```

## Implementation PR slices

This spec is docs-only. Open sensor-surface work it routes:

```text
#316     make the ripr receipt chain real: a machine-readable
         gate-decision receipt, a parser matching what ripr actually
         ships, doctor version pins for ripr/unsafe-review, and loud
         visibility for a configured-but-never-evaluated required gate
#312     proof broker lease edge cases: lease "absent" status,
         base_patch_failed lane routing, manual-cost allowlist path,
         shell-token test gap - the lease half of this surface
#313     coverage sensor transient-failure tracking (exit 101 on the
         PR #305 red run; stayed advisory by policy)
#317-321 xtask precommit and sensor-glue honesty: bounded receipt
         stdout (#317), foreign-ledger skip semantics (#318), pinned-
         version failure reasons (#319), missing-tool receipts that are
         distinguishable (#320) and actionable (#321)
upstream ripr-swarm#1035-1038, unsafe-review-swarm#1516-1518,
         cargo-allow#1467-1470, tokmd-swarm#219-221 - tracked to
         resolution; local glue changes must link them
```

Out of scope here, routed by sibling specs: `[gate].synchronize_mode`
(#306, spec 0003) and per-provider concurrency (#310, spec 0006).

## Release note claim

```text
ub-review emits stable gate, proof, tool, resource, and review artifacts.
```

Concretely claimable for this surface: six core sensors run trigger-scoped
on every pass with status receipts that record the exact command, duration,
and reason; a missing or failed sensor is recorded as an evidence gap, never
as clean evidence; tool metadata is mirrored and verifier-enforced across
`resolved-tools.json` and `tool-status.json`. Not claimable: that
`[tools.*.gate]` thresholds are production-proven (#316), that sensor
version drift beyond tokmd is detected, or that coverage proves anything
beyond execution surface.

## The six reliance questions

What can a user rely on?
The `KNOWN_TOOL_POLICY_KEYS` registry shape and its per-key strip-with-
receipt parsing; the trigger taxonomy and its diff-flag scoping; a
`sensors/<id>/ub-review-sensor-status.json` receipt for every planned sensor
with argv, status, and reason; `stdout.txt`/`stderr.txt` raw receipts; the
tool-status/resolved-tools mirror; `must-run`/`include-if-ready`/
`artifact-only` packet policies with `initial_packet_status` telling lanes
that pending work is unfinished, not missing; lane packets that route only
the sensors a lane consumes.

What can break the gate?
A required sensor's evidence gap when its trigger matched (intelligent-ci
mode only); an evaluated `[tools.*.gate]` threshold that failed; a policy
parse error in a `[tools.*]` section the repo wrote; plus the
`tool_gate_missing_evidence` opt-in on required tools. Today no production
run has ever blocked on a tool-gate threshold (#316).

What is only advisory?
Every non-required sensor outcome (including this repository's coverage and
tokmd), `missing_evidence` tool-gate outcomes under default policy, all
sensor findings as review content, and all successful tool status.

What is visible in the PR?
By default, nothing sensor-shaped. Sensor and provider tables post
`on_failure` only under this repository's `[review_body]` policy; sensor
evidence reaches the PR through the gate verdict and through findings that
survived validation, never as status chatter.

What is artifact-only?
The full sensor receipt tree under `sensors/`, `resolved-tools.json`,
`tool-status.json`, `tool-gate-outcomes.json` and `tool_gate_outcomes.ndjson`,
`work_queue.json`/`work_events.ndjson`, lane packet routing sections, and
the running-summary missing-evidence section.

What does success look like in ten minutes?
Run `ub-review doctor --require-core-tools` on the standard image: six core
tools found, tokmd at 1.12.0. Run `plan --write` on a Rust diff: ripr and
unsafe-review planned with matched triggers, coverage skipped with
`heavy/manual witness requires --allow-heavy` unless leased. Run the gate:
every planned sensor has a status receipt naming its exact command; delete
one required tool from PATH and the run records a `missing` receipt that
becomes a `required-sensor` blocking reason with a receipt pointer in
`review/gate_outcome.json` - an evidence gap with a paper trail, never a
silent pass.