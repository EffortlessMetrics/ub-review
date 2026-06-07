# UB-REVIEW-SPEC-0007 - audit-ci surface

Status: authored 2026-06-06 (release surface spec wave, docs-only).
Child of UB-REVIEW-SPEC-0001. Documents the current behavior of
`ub-review audit-ci`; contract intent is marked as intent. Maturity per the
umbrella: v0 - deterministic judgment only; permissions/secrets/matrix
extraction and the branch-protection query are not yet implemented. Contract
docs: docs/CI_AUDIT_WIZARD.md and docs/adr/0002-single-gate-and-ci-audit-wizard.md;
roadmap item 25 (docs/ROADMAP.md) is delivered in its deterministic v0 form
(#299) - the model-lane acceptance bullet is not yet exercised (lane
unwired) - the first production run target was this repository, and the
#299 report carried the receipts that justified folding standalone CI into
the gate (#300, #301).

## Purpose

audit-ci is the adoption wedge for the single-gate product path. Before a repo
trusts `ub-review/gate` as its required check, audit-ci answers the prior
question with receipts: which of the repo's existing mandatory CI jobs earn
their runner minutes, which should move inside the gate as required proof,
which should become adaptive, and which must stay with a human. The framing is
right-sizing, not downgrading (docs/CI_AUDIT_WIZARD.md). It is a read-only
report: no workflow edits, no branch-protection changes, no PR. The PR-emitting
successor is `setup-ci` (spec 0008, unimplemented).

## User question

```text
Which parts of my existing CI should stay required, become adaptive,
or move elsewhere?
```

## Lifecycle moment

Pre-adoption, and periodically thereafter. audit-ci is a CLI subcommand
(`Command::AuditCi`, src/cli.rs), not an action input - it runs from a shell
in the repo checkout or from a one-off workflow step. It is deliberately
zero-setup: the token argument reads ambient `GITHUB_TOKEN` (not
`UB_REVIEW_GITHUB_TOKEN`) so the token a runner or a `gh` shell already
exports works with no ub-review-specific configuration (comment on
`AuditCiArgs.github_token`, src/cli.rs).

## Consumer

- a maintainer deciding whether and how to adopt the gate, reading
  `audit-report.md`;
- the (future) `setup-ci` migration PR generator, reading the JSON receipts;
- this repo's own governance: the #299 report against ub-review itself
  justified the #301 CI fold (docs/ROADMAP.md, single-gate section).

## Inputs

All flags on `AuditCiArgs` (src/cli.rs):

```text
--root            repo root containing .github/workflows
                  (default ".", env UB_REVIEW_ROOT)
--out             run directory; artifacts land under <out>/ci-audit/
                  (default target/ub-review, env UB_REVIEW_OUT)
--repo            owner/name slug; defaults to GITHUB_REPOSITORY, else the
                  git origin remote (a set-but-empty env is treated as
                  absent, src/main.rs resolve_ci_audit_repo)
--github-token    read-only API token; env GITHUB_TOKEN; tokenless runs
                  degrade to inventory-only
--github-api-url  default https://api.github.com (env UB_REVIEW_GITHUB_API_URL)
--window-days     history window, default 90
```

Local input: `.github/workflows/*.yml|yaml`, line-scanned by
`scan_workflow_text` for triggers, path filters, `timeout-minutes`, and
`uses:` references (src/main.rs). This is a v0 line scan, not a YAML parser.

Remote input (token present): read-only GETs only - `actions/workflows`,
`actions/runs?event=pull_request` filtered to the window, and
`actions/runs/{id}/jobs` (src/main.rs fetch_ci_audit_history). Honest scope
note: history covers pull_request-event runs only; push, schedule, and
release runs are invisible to history, costs, and correlation. YAML-declared
jobs that never ran in the window are still seeded into the inventory so the
security rule can flag release/deploy lanes (src/main.rs
build_ci_audit_artifacts).

## Output artifact / user surface

Everything is artifact-only, written under `<out>/ci-audit/`
(src/main.rs write_ci_audit_artifacts; docs/CI_AUDIT_WIZARD.md):

```text
inventory.json         ub-review.ci_inventory.v1       per-job workflow facts
history.json           ub-review.ci_history.v1         per-job run metrics
costs.json             ub-review.ci_costs.v1           duration percentiles,
                                                       runner-minutes/month
correlation.json       ub-review.ci_correlation.v1     independent-failure
                                                       structure
recommendations.json   ub-review.ci_recommendations.v1 tiered, receipted
audit-report.md        the human-readable report
```

Read-only guarantee: no file outside `<out>/ci-audit/` is created or
modified; the command makes no write API calls. `cmd_audit_ci` creates the
out dir, scans, optionally fetches, builds, and writes - nothing else
(src/main.rs). A clean `git status` after a run is part of the validation
below.

The report orders tier sections by decision relevance - action items first,
human review last: adaptive, move into ub-review/gate, keep required,
advisory, nightly/release, label-gated, human review required, then an
Unclassified catch-all (src/main.rs CI_AUDIT_REPORT_TIER_SECTIONS). Each job
line with run history carries its receipts inline (confidence, p50, run
count, independent failures, runner-minutes/month) plus a short note that
never repeats them; jobs without history render confidence plus their
positioned-to-catch scope instead. Evidence gaps from inventory and history
are de-duplicated into one final "Evidence gaps" section (src/main.rs
render_ci_audit_report).

## Required fields

Per-job, by artifact (struct definitions in src/main.rs; examples in
docs/CI_AUDIT_WIZARD.md):

```text
inventory.json    workflow, job, name, triggers, path_filters, matrix_size,
                  timeout_minutes, permissions, uses_secrets, required_check,
                  required_check_source; top-level evidence_gaps
history.json      job, workflow, window_days, runs, failure_rate,
                  cancellation_rate, flake_rate, rerun_then_pass,
                  evidence_gaps; top-level runs_fetched, pages_fetched,
                  page_cap, run_cap, truncated
costs.json        duration_p50_sec, duration_p90_sec, duration_p99_sec,
                  runner_minutes_per_month, matrix_expansion
correlation.json  independent_failures, co_failing_jobs,
                  cheaper_jobs_compared; top-level independent_failure_rule
                  (the rule text is embedded verbatim in the artifact)
recommendations.json
                  job, workflow, tier, positioned_to_catch, has_caught,
                  receipts[], proposed_policy, confidence, judgment, reason,
                  report_note
```

The independent-failure rule (src/main.rs CI_AUDIT_INDEPENDENT_FAILURE_RULE):
a job counts an independent failure when it fails on a pull_request run while
every cheaper job (lower duration p50 within the same workflow run) passed;
skipped cheaper jobs count as passed.

Receipt cardinality: a job with run history gets three receipts -
`ci-audit/correlation.json#<job>`, `ci-audit/costs.json#<job>`,
`ci-audit/history.json#<job>`. A job without history (tokenless mode, or
never ran in the window) gets exactly `ci-audit/inventory.json#<job>`
(src/main.rs build_ci_audit_artifacts). Every recommendation carries at least
one receipt; no recommendation without a receipt (docs/CI_AUDIT_WIZARD.md).

Tier taxonomy (docs/CI_AUDIT_WIZARD.md):

```text
keep-required                cheap, deterministic, high-signal, foundational
move-to-ub-review-required   every PR, but inside the gate as [[proof.required]]
adaptive                     run when diff class / paths warrant it
label-gated                  expensive witness lane behind a risk-pack label
nightly-release              valuable but not per-PR
advisory                     useful signal, never branch-protection material
flag-for-human               security, secrets, compliance, signing, deploy,
                             provenance, or unclear ownership
```

Honest v0 note: the deterministic classifier emits only three of the seven
tiers - flag-for-human, keep-required, and adaptive (src/main.rs
classify_ci_job_tier). The other four are contract taxonomy, reserved for the
model judgment lane and the setup-ci policy mapping; nothing in v0 produces
them.

The deterministic v0 decision ladder, in order (src/main.rs
classify_ci_job_tier and the CI_AUDIT_* constants):

```text
security pattern match            flag-for-human, confidence high
no run history (tokenless)        flag-for-human, confidence low
fewer than 20 runs in window      flag-for-human, confidence low
duration evidence missing         flag-for-human, confidence low
independent failures > 0          keep-required (high confidence only when
                                  p50 < 300s and runs >= 50, else medium)
p50 >= 300s, 0 independent fails  adaptive (medium only when runs >= 100 and
                                  a sibling job in the same workflow has
                                  proven independent signal, else low)
cheap and quiet                   keep-required, confidence low
```

Security patterns are broad on purpose - codeql, secret, scan, sign,
provenance, attest, deploy, release, publish, permission, push, apply,
terraform, sarif, compliance, docker - matched case-insensitively against
job id, workflow path, workflow name, and `uses:` references. Overmatch is
accepted by design: a false-positive flag costs a human a glance, a
false-negative escape auto-right-sizes a security-sensitive job
(src/main.rs CI_AUDIT_SECURITY_PATTERNS and its comment;
docs/CI_AUDIT_WIZARD.md rules).

## Advisory vs blocking behavior

Entirely advisory. audit-ci writes no gate artifact, has no `fail-on-gate`
coupling, and cannot affect any PR check; its recommendations are input to a
human (and later to setup-ci), never to the gate verdict. The command exits
non-zero only on operational failure: an invalid `--repo` slug, an
unresolvable origin remote, an API fetch failure with a token present, or an
IO error (src/main.rs cmd_audit_ci, resolve_ci_audit_repo,
ci_github_get_json).

Conservatism rules the deterministic tier output (docs/CI_AUDIT_WIZARD.md;
enforced in classify_ci_job_tier): nothing right-sizes below adaptive,
absence of failures alone caps confidence at low, and ambiguity resolves to
flag-for-human.

## Fail-closed behavior

- Tokenless degradation: no token means no API fetch at all. The run
  degrades to inventory-only; a single shared gap line ("no GitHub token:
  run history, durations, and correlation unavailable (inventory-only
  mode)") is mirrored into inventory, history, costs, and correlation so
  each artifact is honest when read standalone; every recommendation tiers
  to flag-for-human at low confidence with receipts limited to
  `inventory.json#<job>`; the report header states inventory-only mode once
  and per-job notes stay empty to avoid repeating it (src/main.rs
  build_ci_audit_artifacts, classify_ci_job_tier).
- Token present but the workflows or runs listing fails: the command errors
  out rather than silently degrading to inventory-only - a half-fetched
  history is never passed off as a complete one (src/main.rs
  ci_github_get_json bails on non-success; fetch_ci_audit_history
  propagates). Per-run job fetches degrade individually: the first three
  failures are itemized as evidence gaps, the rest aggregate into one count
  line.
- History truncation: run fetching caps at 10 pages of 100 runs - 1000 runs
  maximum. `history.json` records `pages_fetched`, `page_cap`, `run_cap`,
  and `truncated: true`, and the shared gap line "run history truncated at
  10 pages / 1000 runs" lands in history, costs, and correlation; the report
  header appends "cap reached" (src/main.rs CI_AUDIT_RUN_PAGE_CAP,
  fetch_ci_audit_history, render_ci_audit_report). Sampling-bias warning:
  the GitHub API returns newest runs first, so a truncated window keeps the
  most recent ~1000 pull_request runs and silently drops older behavior
  inside the stated window. On busy repos, treat truncated rates and
  independent-failure counts as recent-history estimates, not window-wide
  facts.
- Smaller known approximations are receipted, not hidden: runs with more
  than 100 jobs only get their first job page; API items that fail to
  deserialize are counted and reported as dropped; flake_rate and
  rerun_then_pass are workflow-level approximations because jobs are fetched
  for the latest attempt only (src/main.rs fetch_ci_audit_history,
  build_ci_audit_artifacts).
- Thin data never right-sizes: under 20 runs in the window is
  flag-for-human, never adaptive (src/main.rs CI_AUDIT_MIN_HISTORY_RUNS).
- Security-relevant jobs are always flag-for-human; no automatic
  right-sizing, ever, unless the repo opts into an explicit written policy
  (docs/CI_AUDIT_WIZARD.md; src/main.rs classify_ci_job_tier).

## Trust boundary / non-claims

```text
audit-ci reports; it never edits, posts, or gates
recommendations are receipted advice, not verdicts
a quiet job is weak evidence, not proof a job is useless
security jobs are never auto-right-sized
the release never claims "auto-downgrades CI safely" (umbrella never-claims)
```

Honest v0 limits, stated as limits:

- judgment is hardcoded: every recommendation carries
  `judgment: "deterministic"`. The contract reserves
  `judgment: model-assisted` for a bounded model lane that classifies over
  deterministic receipts only; that lane is not wired (src/main.rs audit-ci
  section comment and CiRecommendation construction;
  docs/CI_AUDIT_WIZARD.md rules; docs/adr/0002).
- permissions, secrets, and matrix structure are not extracted from
  workflow YAML: `permissions` is always null, `uses_secrets` always empty,
  and `matrix_size` is inferred from observed API job-name fan-out (default
  1), not parsed from a `matrix:` block. Recorded verbatim as an inventory
  evidence-gap line (src/main.rs build_ci_audit_artifacts).
- branch protection and rulesets are not queried: `required_check` is
  always null and `required_check_source` always "unknown", also recorded
  as an inventory evidence-gap line (src/main.rs). The contract example
  shows `branch-protection | ruleset | unknown`; only "unknown" is real
  today.
- the deterministic classifier emits three of seven tiers (above); the
  contract's survivorship rule is implemented only in its conservative
  direction (sibling-signal check caps adaptive confidence).

None of the open release issues (#306, #310, #312, #313, #317-#321, #347;
#314 and #316 are closed) touch this surface; the audit-ci v0 gaps are
recorded inside the artifacts as evidence_gaps strings and in this spec,
and route through the spec-wave implementation plan (spec 0001).

The six reliance questions:

```text
Rely on:     the six-artifact contract under <out>/ci-audit/ with v1
             schemas; read-only behavior; at least one receipt per
             recommendation; security jobs always flag-for-human;
             tokenless runs degrade loudly, never silently.
Break gate:  nothing. audit-ci has no gate coupling at all.
Advisory:    everything audit-ci emits.
PR-visible:  nothing. audit-ci posts nothing; a workflow may choose to
             upload or summarize the report itself.
Artifact:    all six files; the JSON is the machine surface, the report
             is the human surface.
Ten minutes: from the repo checkout, `ub-review audit-ci --out target/ub-review`
             gives an inventory-only report with explicit gaps; rerun with
             GITHUB_TOKEN exported (e.g. from `gh auth token`) for the full
             history/costs/correlation report. The worked example is this
             repo's own report (#299), whose receipts justified the #301
             CI fold.
```

## Validation commands

```bash
ub-review audit-ci --out target/ub-review            # tokenless: inventory-only
GITHUB_TOKEN=$(gh auth token) ub-review audit-ci --out target/ub-review
                                                     # history mode, 90-day window
git status --porcelain                               # must be empty: read-only proof
cargo test --bin ub-review --locked ci_audit         # artifact schemas, receipts,
                                                     # tokenless degradation, report
cargo test --bin ub-review --locked ci_security_jobs_always_flag_for_human
cargo test --bin ub-review --locked ci_independent_failures_require_all_cheaper_jobs_passing
```

## Implementation PR slices

This spec is docs-only; delivered work is cited, remaining work is routed:

1. Delivered: #299 - the audit-ci read-only report (roadmap item 25), first
   production run against this repository; its receipts were consumed by the
   #300/#301 CI fold and the single-gate dogfood (roadmap item 26).
2. Branch-protection/rulesets query: populate `required_check` and
   `required_check_source` when token scope allows; until then the inventory
   gap line stays. No open issue yet; tracked here and in the artifact gaps.
3. Workflow YAML extraction: permissions, secrets, and matrix structure
   (replacing the line scan or extending it honestly). No open issue yet.
4. Bounded model judgment lane: classify over deterministic receipts only,
   emit `judgment: model-assisted`, never invent facts
   (docs/CI_AUDIT_WIZARD.md rules; docs/adr/0002 judgment split). Off by
   default; deterministic mode remains the supported floor.
5. Remaining tiers (move-to-ub-review-required, label-gated,
   nightly-release, advisory) become emittable only alongside the setup-ci
   policy mapping (spec 0008; roadmap item 28) - a tier the wizard cannot
   turn into a concrete policy line is not worth emitting.

## Release note claim

```text
ub-review can audit existing CI and recommend right-sizing:
a read-only, receipted report of which required checks earn their
runner minutes - deterministic v0, every recommendation cites its
receipt, security jobs always flagged for a human, and every evidence
gap stated in the artifact that carries it.
```
