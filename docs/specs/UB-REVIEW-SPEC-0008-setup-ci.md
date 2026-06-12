# UB-REVIEW-SPEC-0008 - setup-ci surface

Status: authored 2026-06-06 (release surface spec wave, docs-only).
Umbrella: [UB-REVIEW-SPEC-0001](UB-REVIEW-SPEC-0001-release-surface.md).
Maturity: slice 1 implemented - `setup-ci --print-pr` exists
(`Command::SetupCi`, src/cli.rs; `cmd_setup_ci`, src/main.rs). It reads a
prior audit-ci run's five receipts fail-closed (missing artifacts and
schema mismatches are named errors), takes an explicit
`--accept <job>=<command>` list (adaptive tier only; the maintainer
supplies the command because the receipts never record it), renders the
migration plan with the eight PR-body sections to stdout and
`<out>/ci-audit/migration-plan.md`, refuses to invent the
branch-protection remove list while `required_check_source` is `unknown`,
and self-checks the generated `.ub-review.toml` block through the config
loader - any `PolicyError` receipt aborts as a generator failure. No repo
writes, no network, no GitHub calls; output is byte-identical across runs
over the same receipts. `--open-pr` opens the migration PR in its
new-files-only v0: it requires `--action-sha` (the generator refuses to
invent the workflow pin), refuses any repo that already carries a
`.ub-review.toml`, creates one branch plus exactly three new files (the
generated config, the SHA-pinned zero-key gate workflow, the migration
plan under docs/ci/), opens one PR whose body is the plan, never touches
branch protection, and writes `setup-pr-result.json` /
`setup-pr-error.json` receipts under `<out>/ci-audit/`. Still contract
intent: edits to existing workflows or configs (right-sizing diffs), a
branch-protection remove list (prereq A), and
`--apply-branch-protection`; sections below say which side of the line
they are on.

## Purpose

Define the contract for the migration-PR generator before any code exists:
what `setup-ci` reads, exactly which files it may create or edit, what it
must print before it opens anything, what it must never mutate, and the
human-review boundaries that make the PR trustworthy. The command is the
last step of the adoption path in docs/adr/0002: audit-ci produces the
receipts, the gate proves it can fail correctly, and only then does setup-ci
package the fold into one reviewable PR. Writing the contract first means
the implementation is graded against this document, not the other way
around.

## User question

```text
Can ub-review open the migration PR that folds my CI into one gate?
```

Honest answer today: no. The fold has happened exactly once, on this
repository, by hand (roadmap item 26: PRs #300/#301 folded `ci.yml` and
`coverage.yml` into the gate as `[[proof.required]]` tasks and
required/leased tools in `.ub-review.toml`; branch protection was edited
manually via the API). setup-ci is the productized version of that manual
sequence. Until it ships, the answer a release may give is "audit-ci tells
you what to fold and this repo's history shows the fold working; the PR you
write yourself."

## Lifecycle moment

Once per repository adoption, and only after three things are true:

- `audit-ci` has run against the repo and written its receipts (spec 0007;
  implemented today, roadmap item 25);
- the gate verdict surface can fail correctly (`gate_outcome.json`,
  `gate-check`, `fail-on-gate` - implemented, roadmap items 23/24;
  docs/CI_AUDIT_WIZARD.md guardrail: "No replacing CI before gate_outcome
  can fail correctly", sequencing also fixed by ADR 0002's rollout, step 1
  before step 6);
- the maintainer has read the audit report and decided which
  recommendations to accept.

ADR 0002's rollout sequence places setup-ci sixth, "calibrated on the
receipts from steps 3-5" - meaning the audit-ci report, the dogfood fold on
this repo (done), and the rust-test-proof multi-repo rollout (item 27, not
yet done) all feed calibration before the generator ships. setup-ci is a
one-shot adoption event, not a per-PR surface.

## Consumer

```text
the maintainer          reviews and merges the migration PR; the only
                        decision-maker
the repo's existing CI  must pass the migration PR as it stands - the old
                        gate approves its successor (docs/CI_AUDIT_WIZARD.md)
a branch-protection
admin (human)           executes the documented required-checks change after
                        merge, from docs/ci/branch-protection-change.md
a future
--apply-branch-protection
invocation              may execute that same change with an explicitly
                        granted admin token; separate command, never a
                        default (docs/adr/0002)
```

No automation consumes setup-ci output: the PR is a product demo addressed
to a human, and every bullet in it must cite a receipt
(docs/CI_AUDIT_WIZARD.md PR body contract).

## Inputs

All inputs are audit-ci receipts under `<out>/ci-audit/` (spec 0007;
schemas implemented today in src/main.rs):

```text
inventory.json         ub-review.ci_inventory.v1
history.json           ub-review.ci_history.v1
costs.json             ub-review.ci_costs.v1
correlation.json       ub-review.ci_correlation.v1
recommendations.json   ub-review.ci_recommendations.v1
runner-cancellations.json
                       ub-review.ci_runner_cancellations.v1
audit-report.md        the human-readable report
```

plus an explicit acceptance input: the maintainer names which
recommendations the PR implements. The contract phrase is "minimal edits
that implement the accepted recommendations only" (docs/CI_AUDIT_WIZARD.md),
which forces an accept list into the CLI surface; nothing is accepted by
default. The mechanism (flags, a file, or interactive selection) is an
implementation decision for slice 1 below.

Known input gaps the implementation inherits from audit-ci v0, all of which
bound what setup-ci can generate until fixed (spec 0007 carries these):

- `required_check_source` is always `unknown`: the branch-protection /
  rulesets query is not implemented, so audit-ci does not know which checks
  are actually required today. setup-ci cannot write the exact "remove
  required: <old checks>" list without this. Hard prerequisite.
- `permissions` is never extracted and `uses_secrets` is always empty; the
  v0 line scan covers triggers, path filters, timeout-minutes, and `uses`
  only (src/main.rs workflow scan). Security classification therefore rests
  on name patterns (`CI_AUDIT_SECURITY_PATTERNS`), not on observed
  permissions or secret usage. setup-ci must treat this as a reason for
  conservatism, not a license.
- `judgment` is always `deterministic` in v0 (src/main.rs); tokenless runs
  degrade every recommendation to `flag-for-human`. A setup-ci run over a
  tokenless audit has nothing to implement and must say so rather than
  invent edits.
- history caps at 1000 runs with a `truncated` flag; truncation-biased
  receipts cap confidence, and low confidence is a reason not to accept a
  recommendation, which the PR body must surface.

## Output artifact / user surface

The contract (docs/CI_AUDIT_WIZARD.md "setup-ci migration PR contract",
docs/ROADMAP.md item 28). Repo-file changes only:

```text
adds      .ub-review.toml                       required proof + tool gates +
                                                adaptive triggers
adds      .github/workflows/ub-review-gate.yml  the gate workflow, action
                                                pinned by full commit SHA
                                                (consumer contract, README
                                                Bun setup)
adds      docs/ci/ub-review-migration.md        the migration plan
adds      docs/ci/branch-protection-change.md   the exact required-checks
                                                change
edits     .github/workflows/*.yml               right-sized jobs become
                                                non-required / label-gated /
                                                nightly where file-based;
                                                minimal edits implementing
                                                accepted recommendations
                                                only, no broad rewrites
writes    <out>/ci-audit/migration-plan.md      run artifact copy of the plan
```

Delivery modes:

```text
setup-ci --print-pr     renders the full PR contents (file diffs + PR body)
                        without opening anything; no network, no token
setup-ci                opens one PR on a new branch; no force-push
```

What it must never mutate automatically: branch protection and rulesets.
The migration PR cannot change branch protection because that is an admin
API surface, not a repo file (docs/adr/0002); the PR carries the exact
change as text. A later `setup-ci --apply-branch-protection` may apply it
with an explicitly granted admin token - a separate explicit command
invocation, never a default, never bundled into the PR run
(docs/CI_AUDIT_WIZARD.md, docs/adr/0002).

## Required fields

PR body structure, in this order (docs/CI_AUDIT_WIZARD.md; the audit-ci
report reuses these section headings in its own decision-relevance order,
action items first and human-review last - src/main.rs
`CI_AUDIT_REPORT_TIER_SECTIONS` - so the report previews the PR sections
without matching their order):

```md
## Decision
## Keep required
## Move into ub-review/gate
## Right-size to adaptive
## Label-gated / nightly / release
## Human review required
## Proposed branch protection change
## Rollback
```

Every bullet cites a receipt: duration percentiles, independent failures,
diff class, risk class, or policy reason - receipt pointers of the form
`ci-audit/correlation.json#<job>` as recommendations.json already carries
them. No recommendation without a receipt; no vibes
(docs/CI_AUDIT_WIZARD.md rules).

`docs/ci/branch-protection-change.md` must state the exact change:

```text
remove required: <the old check contexts, by exact name>
add required:    ub-review/gate
```

plus where to apply it (branch protection or ruleset, as discovered by the
audit) and the rollback inverse. The required-check name comes from
`[gate].required_check`, default `ub-review/gate` (src/config.rs
`GateConfig`).

The Rollback section must be complete: reverting the migration PR restores
prior CI behavior entirely, and the branch-protection rollback names the
exact contexts to re-add (roadmap item 28 acceptance; the live precedent is
roadmap item 26's recorded rollback for this repo).

The generated `.ub-review.toml` must:

- express accepted `move-to-ub-review-required` jobs as `[[proof.required]]`
  entries with `id`, `command`, `reason`, `languages`, `diff_classes` (the
  shapes implemented today, src/config.rs `RequiredProofPolicy`);
- express accepted `adaptive` jobs as trigger-scoped tool or proof policy,
  carrying the proposed_policy text from recommendations.json;
- round-trip through the config loader with zero `PolicyError` receipts -
  a generator that emits keys the loader strips has failed;
- never emit reserved or deprecated keys: no `[providers]` section
  (reserved, unwired - src/config.rs), no legacy
  `[gate].synchronize_mode` (stripped with a deprecation `PolicyError`,
  #306). Generating documentation-only knobs into a consumer repo would
  launder intent as behavior;
- only propose `[tools.<id>.gate]` thresholds whose receipt chain is real:
  the ripr threshold qualifies since #335 (#316 closed) — the sensor
  produces `sensors/ripr/gate-decision.json` and the threshold has blocked
  production PRs (#342, #346). The principle stands for every other tool:
  a generated threshold whose sensor emits no gate-decision receipt is
  decorative policy text, and the generator must not ship decorative
  policy.

## Advisory vs blocking behavior

The setup-ci surface itself is entirely advisory by construction: it
proposes, a human accepts, and nothing it emits changes any gate behavior
until the PR is merged. The handoff is the point - once merged, the
generated `.ub-review.toml` becomes blocking policy under the gate
semantics of spec 0003 (authored in this wave), and the branch-protection
change (applied by a
human, or later by `--apply-branch-protection`) makes `ub-review/gate` the
required check.

Human-review boundaries the contract fixes:

- security-sensitive jobs (CodeQL, secret scanning, provenance, signing,
  deploy gates, permission checks - `CI_AUDIT_SECURITY_PATTERNS`) are
  always `flag-for-human` and are never auto-right-sized; setup-ci must not
  touch their workflow files at all, ever, unless the repo opts in via an
  explicit written policy (docs/CI_AUDIT_WIZARD.md rules, roadmap item 28
  acceptance);
- the survivorship rule binds the generator as it binds the auditor:
  absence of failures alone caps confidence at low, and a right-size needs
  both a weak `has_caught` record and a `positioned_to_catch` statement
  plausibly covered elsewhere (docs/CI_AUDIT_WIZARD.md, docs/adr/0002);
- the migration PR must pass the repo's pre-existing CI as it stands before
  any branch-protection change is applied - the old gate approves its
  successor (docs/CI_AUDIT_WIZARD.md constraints, roadmap item 28
  acceptance). setup-ci edits therefore cannot break the checks they are
  retiring.

## Fail-closed behavior

Contract intent (nothing here is implemented):

```text
missing or schema-invalid ci-audit receipts      refuse to run; name the
                                                 missing artifact
recommendation without a receipt pointer         excluded from the PR; named
                                                 in the plan as excluded
accepted recommendation not present in
recommendations.json                             error, not silent skip
all recommendations flag-for-human
(tokenless audit, thin history)                  emit no edits; --print-pr
                                                 explains why there is no PR
                                                 to open
required-checks state unknown
(required_check_source: unknown)                 refuse to write
                                                 branch-protection-change.md
                                                 with invented contents;
                                                 missing evidence is missing
                                                 evidence
generated .ub-review.toml produces any
PolicyError receipt on reload                    generator failure, abort
no token / no push permission                    --print-pr still works; PR
                                                 opening fails with the
                                                 named permission gap
--apply-branch-protection without an
admin-scoped token                               hard error before any API
                                                 write; partial application
                                                 is worse than none
```

The product invariant carries over from the rest of the pipeline: missing
evidence is recorded as missing evidence, never as clean evidence, and
ambiguity resolves toward no edit and `flag-for-human`.

## Trust boundary / non-claims

```text
setup-ci proposes; the maintainer decides.
The PR never mutates branch protection.
Security jobs are never auto-right-sized.
No recommendation without a receipt.
The old gate approves its successor.
```

Never claim (umbrella 0001 forbidden claims): "auto-downgrades CI safely".
setup-ci does not validate that the new gate catches everything the
right-sized jobs were positioned to catch; the survivorship rule and the
human review are the mitigations, not a proof. The framing is right-sizing,
not downgrading (docs/CI_AUDIT_WIZARD.md), and the PR must read that way.

The gate it migrates repos onto has its own honest limits a maintainer
inherits: the proof broker has known edge cases around lease absence and base-patch
failure routing (#312), and sensors can fail transiently and recover (the
coverage exit-101 case, #313, stayed advisory by policy). Spec 0003 (gate
surface) carries those in full; the migration plan doc should link them
rather than restate them, because a migration PR that oversells its
destination violates the no-vibes rule.

And the largest non-claim of all, restated from the Status line: none of
this exists. A release, a README, or a PR comment must not present setup-ci
as available until the slices below land.

## Validation commands

Slice-1 validations, runnable today:

```bash
ub-review audit-ci --out target/ub-review     # the input side
ub-review setup-ci --print-pr --out target/ub-review
  # renders the migration plan; byte-identical on repeat runs over the same
  # receipts; only file write is <out>/ci-audit/migration-plan.md; zero
  # network calls
ub-review setup-ci 2>&1 | head -2   # names the unimplemented PR-opening
                                    # slice and says to pass --print-pr
cargo test --bin ub-review --locked setup_ci  # fail-closed paths + the
                                              # round-trip oracle
cargo xtask policy-check            # this spec is a governed docs surface
```

Future acceptance commands, runnable only once the later slices land (the
acceptance criteria of roadmap item 28 turned into commands):

```bash
ub-review setup-ci --out target/ub-review --accept <ids>
  # opens exactly one PR; git diff against the branch touches only the
  # contract-listed files; security-pattern workflows untouched
# generated-config round-trip: load the emitted .ub-review.toml and assert
# zero PolicyError receipts (inline test, not a shell command)
# CI proof: the opened migration PR goes green under the repo's
# pre-existing required checks before any protection change
```

## Implementation PR slices

This section is the build plan for the command. Each slice is one
review-fast PR with verifier or test coverage for any new artifact.

```text
prereq A  audit-ci branch-protection / rulesets query: populate
          required_check and required_check_source for real (today always
          unknown). Without this, branch-protection-change.md cannot name
          the old required checks. Lands in the audit-ci surface
          (spec 0007).
prereq B  audit-ci permissions + uses_secrets extraction from workflow
          YAML, hardening the security flag beyond name patterns
          (spec 0007).
prereq C  item 27 calibration: the rust-test-proof rollout produces the
          multi-repo receipts ADR 0002 says setup-ci is calibrated on.

slice 1   setup-ci skeleton + --print-pr only. New Command variant and
          cmd_setup_ci; read and schema-validate the five ci-audit
          receipts; take an explicit accept list; render migration-plan.md
          and the PR body (Decision ... Rollback) to stdout and
          <out>/ci-audit/migration-plan.md. No repo writes, no network.
          Fail-closed paths from this spec land here with tests.
slice 2   .ub-review.toml generation. Accepted move-to-ub-review-required
          recommendations become [[proof.required]] entries; accepted
          adaptive recommendations become trigger policy; required tools
          carried over. Round-trip test: reload emits zero PolicyError
          receipts; assert the generated file contains no [providers] and
          no legacy synchronize_mode (#306) and proposes no threshold whose
          sensor emits no gate-decision receipt (the ripr chain is proven,
          #335; every other tool's is not).
slice 3   gate workflow + docs generation: ub-review-gate.yml pinned by
          full commit SHA, docs/ci/ub-review-migration.md, and
          docs/ci/branch-protection-change.md built from prereq A's
          required-checks data (exact remove list, add ub-review/gate,
          rollback inverse).
slice 4   minimal workflow edits. File-based right-sizing only: trigger
          narrowing, label gates, nightly schedules, required-job removal
          where expressible in YAML. Hard test: security-pattern workflows
          are byte-identical before and after; diff surface limited to
          accepted recommendation ids.
slice 5   PR opening. One branch, one PR, no force-push, token-gated;
          --print-pr remains the no-network path and prints exactly what
          slice 5 would open. Acceptance run on a real repo: the PR passes
          that repo's pre-existing CI.
slice 6   setup-ci --apply-branch-protection (separate, later). Admin
          token explicitly granted; refuses unless the migration PR is
          merged and the gate has a green run on the default branch;
          applies exactly the change in branch-protection-change.md and
          prints the rollback command. Never a default; never bundled
          into slices 1-5.
```

## Release note claim

Today: nothing. A release must not name `setup-ci` as a command, and the
umbrella claim "ub-review can fold required CI checks into one gate" is
backed today by the manual dogfood fold (roadmap item 26), not by this
surface.

After slices 1-5 land and the acceptance run on a real repo is green, the
claimable sentence is:

```text
ub-review setup-ci opens one migration PR - config, gate workflow, minimal
right-sizing edits, and the exact branch-protection change spelled out -
with every recommendation citing an audit receipt. It never touches branch
protection itself.
```

Still never claimable, at any maturity: "auto-downgrades CI safely".

## The six reliance questions

What can a user rely on?
Today: only the contract in this spec, docs/CI_AUDIT_WIZARD.md, and roadmap
item 28 - the file list, the PR body order, the guardrails, and the promise
that branch protection is never auto-mutated. There is no behavior to rely
on; relying on the command existing is an error until the slices land.

What can break the gate?
Nothing in this surface. setup-ci output is inert until a human merges the
PR; after merge, the generated `.ub-review.toml` participates in gate
semantics under spec 0003, and any red it causes is that spec's contract,
not this one's.

What is only advisory?
Everything setup-ci emits: the PR, the plan, the docs, the proposed
branch-protection change. The accept list is the only mechanism by which a
recommendation becomes an edit, and `flag-for-human` recommendations never
become edits at all.

What is visible in the PR?
The migration PR is the entire user surface: receipt-cited body sections
from Decision through Rollback, the four added files, and minimal workflow
diffs. `--print-pr` shows the identical content with nothing opened.

What is artifact-only?
`<out>/ci-audit/migration-plan.md` (the run-artifact copy of the plan) and
the upstream audit receipts it was built from.

What does success look like in ten minutes?
Future tense, by contract: run `audit-ci` with a token, read the report,
run `setup-ci --print-pr`, see a PR whose every bullet carries a receipt
pointer you can open, whose workflow diffs touch only the jobs you
accepted, and whose branch-protection doc names the exact checks to remove
and the one check to add. Accept, open, watch the repo's own old CI go
green on the PR that retires it. Today, the ten-minute version is: run
`audit-ci`, read the report, and write that PR yourself - which is exactly
what this repository did (roadmap item 26), and why the contract above is
written from receipts rather than hope.
