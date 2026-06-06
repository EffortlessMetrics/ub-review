# ub-review init / align: onboarding and careful CI de-bloat

Status: design, docs-only. Consolidates [#327](https://github.com/EffortlessMetrics/ub-review/issues/327)
and its brownfield comment.
Related: the read-only audit and migration-PR contracts in
[CI_AUDIT_WIZARD.md](CI_AUDIT_WIZARD.md), the decision record
[adr/0002-single-gate-and-ci-audit-wizard.md](adr/0002-single-gate-and-ci-audit-wizard.md),
the execution model in
[#325](https://github.com/EffortlessMetrics/ub-review/issues/325), and the gate
surface in
[specs/UB-REVIEW-SPEC-0003-intelligent-ci-gate.md](specs/UB-REVIEW-SPEC-0003-intelligent-ci-gate.md).

`init` / `align` is the onboarding motion for the single-gate design. `audit-ci`
and `setup-ci` are the read-only report and the migration-PR generator under it;
this doc defines the onboarding command those surfaces serve and the runtime
model the migration converges toward. It proposes and writes a reviewable plan;
it never silently rewrites CI.

## Purpose

Onboard any repository to one tight CI gate: one runner, one required check —
the deterministic core floor — with `ub-review` riding along advisory, model
lanes reasoning on top, self-hosted-primary with GitHub-hosted overflow. The
goal from ADR 0002 stands: `ub-review/gate` becomes the standard and only
required PR check, efficient and good enough out of the box to deserve that
position.

Two entry points:

```text
init   (greenfield, generate): inspect a codebase with no fitting gate yet —
       languages, workspace shape, relevant sensors — and generate a tuned
       single-gate ci.yml plus .ub-review.toml. Compose existing good tools;
       do not reinvent them.

align  (brownfield, ingest + align): ingest a repo's EXISTING CI stack and
       propose how to converge it onto the single gate, as a reviewable diff
       a maintainer can read, trust, and merge.
```

Both produce the same artifact shape: a migration plan plus the proposed
workflow as a diff. `align` is the harder, more important half, because it
must not break anything load-bearing while it de-bloats.

## The de-bloat thesis

Most CI designs are bloated by accretion: a pile of parallel required checks,
redundant tools that catch the same failure mode, a separate runner per check,
the whole heavy set run on every PR. Each entry was reasonable when it landed;
the sum is slow, expensive, and noisy, and nobody prunes it because deleting a
check feels dangerous.

The single-gate design is tighter: one runner, a deterministic floor running
under the model latency, advisory model lanes on top, and diff-relevant extras
only when the diff warrants them. `init` / `align` migrates a repo toward that
shape. The framing is **right-sizing, not downgrading**: less fixed CI, more
useful proof.

## The runtime model: two lists, one runner

The output of onboarding is two lists and a runtime rule that draws from them.
This is the backbone of the whole design.

### 1. Tight required list — the deterministic floor that always blocks

The small mandatory set that must ALWAYS run and BLOCK the merge: fast,
load-bearing, deterministic. It is the single required check (`ub-review/gate`)
and nothing else is branch-protection material. Keep it tight — every entry
here taxes every PR, so it earns its place by being cheap, deterministic, and
genuinely load-bearing. Its failure is the only thing that turns the gate red.

```text
required list  -> runs on every PR, every head SHA
               -> deterministic, no model output in the verdict
               -> failure = red gate (the one hard block)
               -> kept tight on purpose
```

### 2. Longer structured suggested list — the catalog, never mandatory

Everything else: classified, cataloged, but not mandatory. These are the
checks and sensors a run *may* execute. The list is **structured, not a flat
dump** — each entry carries enough metadata that the runtime can select from
it intelligently:

```json
{
  "id": "windows-e2e",
  "checks": "Windows-specific runtime regressions on release paths",
  "covers": ["platform-specific UB", "path-handling divergence"],
  "cost": "heavy",
  "relevant_when": {"diff_classes": ["source-general"], "paths": ["src/os/**"]},
  "origin": "folded from .github/workflows/e2e.yml job e2e-matrix-windows",
  "blocking": false
}
```

`checks` / `covers` / `cost` / `relevant_when` let the runtime pick the right
fills for a given diff instead of running everything. Nothing in this list
blocks the merge.

### 3. The LLM fills the runner during investigation

At runtime the required list always runs. Then, while the model lanes
investigate the diff — the latency window the runner would otherwise spend
idle — the LLM dynamically selects items from the suggested list to fill the
runner's spare capacity: diff-relevant, opportunistic, advisory. The runner
stays busy with useful work under the model latency, and none of those fills
can block the merge.

```text
t0  ┌──────────────────────────────────────────────┐
    │ required list (deterministic floor)  -> verdict │  always; blocks
    ├──────────────────────────────────────────────┤
    │ model lanes investigate the diff ............. │  advisory
    │   LLM selects fills from the suggested list:   │
    │   diff-relevant sensors / checks run here      │  advisory; never blocks
t1  └──────────────────────────────────────────────┘
        one runner, one required check
```

This is the same "build shared context once, overlap model investigation with
local proof" stance from [WHY_THIS_DESIGN.md](WHY_THIS_DESIGN.md), applied to
onboarding: the suggested list is the menu the overlap draws from.

The careful de-bloat analysis exists to produce exactly these two lists from a
bloated CI: collapse a pile of parallel required checks into ONE tight required
floor plus a structured suggested catalog the LLM draws from.

### Budget-bounded fills: target and cap

The suggested-fill selection is time-budgeted and configurable per repo. Two
knobs bound it:

```text
target   soft per-run budget the fills aim for      default 30 min
cap      hard ceiling; the job timeout               default 60 min
```

Runtime rule: the tight required floor always runs. The LLM then fills the
runner from the structured suggested list — selecting diff-relevant items — to
approach the target, and **never exceeds the cap**. As elapsed time approaches
the target it stops scheduling new suggested work; the cap is the absolute
ceiling that bounds both cost and wall-clock. A fill already running near the
cap is the timeout's job to stop, not a new fill's.

```text
0 ───────────────── target (30m) ──────────── cap (60m, job timeout)
│ required floor │ LLM-selected fills approach target │  hard stop
│ always         │ stop scheduling new fills near target │  ceiling
```

So the three-part model is budget-bounded: required (always) + LLM-selected
suggested fills (up to the target, hard-capped). This is how the runner stays
usefully busy under the model latency without unbounded cost or runtime.

`init` / `align` proposes `30 / 60` as sensible defaults, documents that both
are configurable, and the generated `ci.yml` sets the job `timeout-minutes` to
the cap.

#### Why 30 / 60: the economic anchor

The defaults are chosen so budget-bounded multi-step model review is affordable
on **every** PR, not so they cap an expensive exception. At the 30-minute
target with ~2 runs per PR, per-PR cost lands around **$0.50** at API pricing
(MiniMax-M3 primary, OpenCode deepseek-v4-flash fallback). That ~$0.50/PR is
the design point: cheap enough to run on every PR rather than reserve for big
diffs. It pairs with the observed heavy-use figure — under ~2% of a weekly
budget across multi-day heavy use.

The knobs are configurable and the cost scales with them: raising the target
raises per-PR cost roughly proportionally (a 60-minute target is ~2x the run
budget, hence ~2x per-PR cost). A repo that wants deeper review on every PR
pays for it predictably; a repo that wants the floor near-free keeps the
default and stays around $0.50/PR.

## Careful-analysis methodology

De-bloating CI correctly is dangerous. You cannot just delete checks: a check
that has "caught nothing in 90 days" may be the only thing positioned to catch
a rare, severe failure. The analysis classifies **each** existing check or job
without breaking anything load-bearing, and every classification feeds one of
the two lists (or, rarely, the drop pile).

| Classification | Meaning | Lands in |
|---|---|---|
| load-bearing deterministic floor | cheap, deterministic, high-signal; its failure must still block | **required list** |
| coverable by a sensor | a composed sensor already catches this failure mode (bespoke unsafe scan -> `unsafe-review`; lint job -> the lint sensor) | **suggested list** (folded into the one job) |
| advisory candidate | useful signal, not branch-protection material; move from blocking to advisory, model-reviewed | **suggested list** (advisory-first) |
| diff-relevant extra | valuable when the diff/paths warrant it, wasteful on every PR | **suggested list** (with `relevant_when`) |
| redundant | a good existing tool already covers the exact same failure mode (no NIH) | **dropped — only after coverage is confirmed elsewhere** |
| uncertain / unknown failure mode | purpose or coverage cannot be established | **required list — kept blocking, flagged for human** |

Mapping the per-check classes onto the two lists:

```text
load-bearing            -> required list
coverable / advisory /
  diff-relevant         -> suggested list (folded, advisory-first, diff-gated)
confirmed-redundant     -> dropped (only with proven coverage elsewhere)
unknown / load-bearing
  but unclear           -> stays in the required list; flagged for human
```

Rules (non-negotiable):

- **Never drop a check whose failure mode is not demonstrably covered
  elsewhere.** Absence of past failures is not coverage. Default to keeping a
  check whose purpose cannot be established.
- **Unknown stays required.** A check with an unclear or unestablished failure
  mode is kept blocking and surfaced for human review, never quietly demoted.
- **Preserve required-check NAMES.** Branch protection maps checks by name;
  keep the names stable so protection keeps mapping while the work moves
  inside the gate. The exact required-checks change is spelled out for the
  maintainer, never applied silently (see ADR 0002 and
  [CI_AUDIT_WIZARD.md](CI_AUDIT_WIZARD.md)).
- **Advisory-first.** Do not newly-block on the model. Moving a check to
  advisory must not make the gate red on model output; only the deterministic
  floor blocks.
- **Reviewable diff, never a silent overwrite.** The output is a diff against
  the current workflow plus a rationale, not an in-place rewrite.
- **Surface every "kept because unclear."** Each one is a decision the human
  makes, not the tool.
- **Security stays human.** Security, secrets, signing, deploy, provenance,
  and compliance jobs are flagged for human review, never auto-right-sized —
  consistent with the `flag-for-human` tier in ADR 0002.

This is the survivorship discipline from ADR 0002 stated as an onboarding
procedure: the plan must say both what a job is positioned to catch and what
it has caught, and conservative defaults win ties.

## File-driven mode

The model lanes are one driver, not the only one. If a repo has no PR model
providers configured — no MiniMax, no OpenCode keys — `init` / `align` does not
stall and does not silently skip the analysis. It emits the inspection / ingest
findings plus the migration plan as a self-contained file an external coding
agent (Claude Code, Codex) runs off to execute the convergence.

```text
two drivers, one plan:
  ub-review lanes      model lanes select suggested fills and reason in-runner
  external agent       the same plan, written to a file, executed by Claude/Codex
```

Same migration plan, same two lists, same careful classification. The driver
changes; the plan and its boundary do not. This keeps the no-model tier from
ADR 0002 a real product tier, not a degraded run: deterministic findings and a
complete, executable plan with zero tokens.

## Output and documentation

`init` / `align` writes a migration plan and the proposed `ci.yml` as a
reviewable diff. The plan is the per-check classification table — every
existing check, its class, the list it lands in, and a short rationale per
decision — plus the resulting required list and structured suggested catalog.

```text
migration plan
  per-check classification table  every job -> class -> list -> rationale
  required list                   the tight deterministic floor
  suggested list                  structured catalog (checks/covers/cost/when)
  budget defaults                 target 30 min / cap 60 min, both configurable
  proposed ci.yml                 as a diff against the current workflow;
                                  job timeout-minutes set to the cap
  branch-protection change        exact required-checks edit, stated not applied
  kept-because-unclear            the human-decides queue, surfaced explicitly
```

Good documentation is a first-class requirement, not a byproduct. A maintainer
must be able to read the plan and understand exactly what changed, why each
check moved where it moved, and trust the result enough to merge it. A
migration the maintainer cannot follow is a failed migration regardless of how
correct the underlying classification is.

### Worked example: unsafe-review-swarm #1524

`unsafe-review-swarm`
[#1524](https://github.com/EffortlessMetrics/unsafe-review-swarm/issues/1524)
is the reference migration: a routed, multi-lane, size-routed gate converged to
one tight gate.

```text
before: size-routed required lanes
  small-pr-gate     required   fmt + check + clippy + unit
  medium-pr-gate    required   + integration + doc
  large-pr-gate     required   + e2e matrix + coverage + extended sensors
  (the heavy set became required as the PR grew; three runners' worth of
   mandatory work, much of it redundant across the size tiers)

after: one tight gate
  required list (always, blocks):
    ub-review/gate  -> fmt, check, clippy, unit as [[proof.required]]
                       (load-bearing deterministic floor; names preserved)

  suggested list (LLM fills during investigation, advisory, never blocks):
    integration     coverable        relevant_when source touched
    doc             coverable        relevant_when docs/public API touched
    e2e matrix      diff-relevant    relevant_when os/platform paths touched
    coverage        advisory         folded as advisory receipt
    extended sensors -> unsafe-review / ripr / cargo-allow (composed, no NIH)

  dropped: only the lanes whose failure mode was confirmed redundant with a
           composed sensor; everything uncertain stayed in the required list.
```

The size-routing disappears because the runtime model replaces it: the required
floor is constant and tight, and the LLM pulls the right fills from the
suggested list per diff instead of a coarse size tier escalating the whole
mandatory set.

## Boundary

`init` / `align` is bounded the same way the rest of `ub-review` is:

- It **proposes and writes a reviewable plan and diff**; it does not apply
  changes. No silent overwrite of any workflow.
- **Advisory-first**: the deterministic core floor stays the only hard gate;
  nothing newly-blocks on the model.
- **No committed secrets.** Provider keys and tokens are referenced, never
  written into generated files.
- **No branch-protection mutation.** The exact required-checks change is
  stated for the maintainer to apply (or a later explicit, separately-invoked
  apply step), never changed by default.
- **No proof claims.** Consistent with `unsafe-review` posture, `init` /
  `align` makes no claim of proven soundness, UB-free status, Miri
  cleanliness, or site execution. It right-sizes CI; it does not certify code.
- **Conservative by default.** Unknown stays required; redundant is dropped
  only with confirmed coverage; missing evidence is reported as missing
  evidence, never as clean evidence.
- **Budget-bounded.** Suggested fills aim for the target and never exceed the
  cap (job timeout); cost stays bounded and predictable (~$0.50/PR at the
  defaults) rather than open-ended.

The single tight required gate is the product. `init` / `align` is how a repo
gets there without losing anything load-bearing on the way.
