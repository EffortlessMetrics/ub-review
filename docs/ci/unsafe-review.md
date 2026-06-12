# unsafe-review CI integration

`unsafe-review` is a static unsafe-contract review signal. CI should treat it as
review evidence first and as a blocking policy only when the repository has
explicitly opted in.

## Pull requests

Recommended PR command surface:

```bash
cargo xtask unsafe-review-pr
```

The PR job should upload:

```text
target/unsafe-review/cards.json
target/unsafe-review/pr-summary.md
target/unsafe-review/github-summary.md
target/unsafe-review/cards.sarif
target/unsafe-review/comment-plan.json
target/unsafe-review/witness-plan.md
target/unsafe-review/lsp.json
target/unsafe-review/receipt-audit.json
```

Default PR status is advisory. Missing cards or failed execution should be
reported as missing review evidence.

## Risk PRs

If a PR changes high-risk unsafe seams, require a witness route before merge.
Examples include FFI boundaries, raw pointer dereference, layout/representation
assumptions, aliasing-sensitive conversions, allocator ownership transfer, and
lifetime extension.

A witness route may be a targeted test, Miri run, sanitizer run, fuzz/workload
receipt, or another documented execution path. It is evidence for review, not a
proof of soundness.

## Nightly and release

Nightly jobs should run concrete unsafe witnesses where feasible:

```bash
cargo xtask unsafe-review-repo
cargo +nightly miri test
cargo mutants
```

Release jobs should fail only on explicit repository policy, such as unexplained
new unsafe-review gaps on public or load-bearing crates.

Keep retained suppressions in `policy/allow.toml` by default. Split them into a
dedicated unsafe-review ledger only after the repo has enough real entries that
the separate file makes review easier.

## ub-review orchestration

`ub-review` routes unsafe-review output to the UB, tests, architecture,
opposition, and security lanes. Those lanes use the cards and witness plan to
ask whether changed unsafe code is reviewable, while runtime backstops continue
to live in focused test, Miri, sanitizer, mutation, and coverage jobs.

### Wired artifact routing (unsafe-review 0.3.4+, #359)

`ub-review` invokes `unsafe-review first-pr --out-dir <sensor-dir>/unsafe-review-output`
so the structured artifact bundle is written to a known location. `first-pr`
uses `--out-dir`; `--out` belongs to other unsafe-review subcommands and does
not place this bundle in the sensor directory. The bundle files (tracked by
`sensor_outputs`, included in `resolved-tools.json`) match the real
`first-pr --out-dir` manifest's `artifacts` map — note `receipt-audit.md` and
`pr-summary.md` are Markdown:

```text
sensors/unsafe-review/unsafe-review-output/unsafe-review-gate.json
sensors/unsafe-review/unsafe-review-output/cards.json
sensors/unsafe-review/unsafe-review-output/comment-plan.json
sensors/unsafe-review/unsafe-review-output/repair-queue.json
sensors/unsafe-review/unsafe-review-output/receipt-audit.md
sensors/unsafe-review/unsafe-review-output/review-kit.json
sensors/unsafe-review/unsafe-review-output/pr-summary.md
sensors/unsafe-review/unsafe-review-output/cards.sarif
sensors/unsafe-review/unsafe-review-output/lsp.json
sensors/unsafe-review/unsafe-review-output/policy-report.json
```

`unsafe-review-gate.json` is the top-level manifest (schema
`unsafe-review-gate/v1`). Its real shape:

```json
{
  "schema_version": "unsafe-review-gate/v1",
  "dialect": "unsafe-review",
  "status": "advisory",
  "summary": { "new_gaps": 0, "worsened_gaps": 0, "resolved_gaps": 0, "inherited_gaps": 0 },
  "artifacts": { "cards": "cards.json", "comment_plan": "comment-plan.json", ... },
  "trust_boundary": "static unsafe-review coverage evidence; not proof, not a merge verdict",
  "tool": "unsafe-review", "tool_version": "0.3.4"
}
```

Two contract details ub-review depends on: the movement counts are **nested
under `summary`** (not flat top-level), and the `artifacts` map keys are
**snake_case** (`comment_plan`, `repair_queue`, …) while their values are the
hyphenated filenames.

ub-review routes by `schema_version` before binding the typed shape: only
`unsafe-review-gate/v1` is parsed. Absent, unreadable, malformed, or unknown
gate artifacts become typed evidence gaps naming the failure; an unknown
version names the found version. The `trust_boundary` field is preserved and
surfaced verbatim in every lane packet and in the shared context.

**Lane packets** (`lanes/<lane>.md`, "Routed sensor evidence" section): when
unsafe-review's sensor status is `ok` and `unsafe-review-gate/v1` is parsed,
each receiving lane (`ub-memory-lifetime`, `security`, `compiler`) gets the
movement summary (`summary.new_gaps`, `summary.worsened_gaps`,
`summary.resolved_gaps`, `summary.inherited_gaps`) and the comment-plan
candidates loaded by following the `comment_plan` artifacts pointer.

**Shared context** (`review/shared_context.md`): an "unsafe-review Coverage
Evidence" section is added between Sensor Statuses and Initial Work Queue. It
includes the tool/version provenance, advisory status, movement summary,
candidate count, and comment-plan entries rendered as JSON for audit.

**Schema routing and evidence gaps**: `schema_version ==
"unsafe-review-gate/v1"` means full structured evidence. Any other value, or
an absent/malformed file while the sensor status is `ok`, becomes an
`artifact-gap` entry in `missing_or_failed_sensor_evidence`. Required
unsafe-review sensors therefore block from the same receipt chain as other
required sensor gaps. Markdown output is never scraped as a substitute.

**Compiler intake**: `comment-plan.json` entries are deserialized into a
structured type (carrying `card_id`, `path`, `line`, `changed_line`,
`coverage_gap`, `selection_reason`, `selection_reason_code`,
`confirmation_state`, `trust_boundary`) and routed into the review compiler as
inline-comment candidates. They share the same `max_inline_comments` cap,
RIGHT-side diff-line guard, candidate ledger, dedupe, and refuter path as model
lane candidates. Tools do not post inside ub-review mode.

### Trust boundary

unsafe-review's `trust_boundary` sentence ("static unsafe-review coverage
evidence; not proof, not a merge verdict") is surfaced unchanged. Sensor output
is review context only; the deterministic floor (required-sensor gap logic,
`[tools.*.gate]` thresholds) decides gate outcomes. A populated comment-plan
does not affect the gate verdict.
