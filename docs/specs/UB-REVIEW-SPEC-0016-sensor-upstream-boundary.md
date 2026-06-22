# UB-REVIEW-SPEC-0016 — sensor-defect upstream boundary

Status: authored 2026-06-22 (Wave 6+, docs-only).
Umbrella: [UB-REVIEW-SPEC-0001](UB-REVIEW-SPEC-0001-release-surface.md).
Related: [SPEC-0005](UB-REVIEW-SPEC-0005-sensor-tool-integration.md) (sensor
tool integration — this spec owns the upstream-governance contract that
SPEC-0005's "Trust boundary" section sketches), [ADR 0001](../adr/0001-whole-runner-stewardship.md).
Maturity: production — the upstream-boundary rule is enforced by convention
and dogfooding; this spec consolidates the policy that was previously
scattered across five docs into one canonical statement.

## Purpose

Own the canonical rule for sensor-defect governance: when a sensor
(`tokmd`, `cargo-allow`, `ripr`, `unsafe-review`, `ast-grep`, `actionlint`)
exposes a real defect or weak contract, it is filed upstream in the matching
repo — never silently absorbed into local `ub-review` glue. Previously this
rule was repeated (with the same routing table) across five docs:

- `docs/ARCHITECTURE.md` (line 60)
- `docs/SENSOR_ROUTING.md` (line 29)
- `docs/CODEX_FINISH.md` (line 52)
- `docs/RUNNER_IMAGE.md` (line 161)
- `docs/REQUIREMENTS.md` (line 15)

This spec is the single source of truth. The other docs now carry a one-line
cross-reference to it.

## The rule

> **Sensor defects are filed upstream in the matching repo, not silently
> absorbed into local `ub-review` behavior.** Local workarounds are allowed
> only to keep `ub-review` usable, and must link the upstream issue. Never
> fork sensor behavior silently.

This applies to all sensors — not just the Bun UB hunt sensors. A defect in
the sensor's command, output contract, finding shape, or suppression semantics
belongs upstream.

## Per-sensor upstream routing

| Sensor | Defect class | Upstream repo |
|---|---|---|
| `ripr` | bug or weak command/output contract; suppression-schema limitations; move-detection / `cfg(test)` skipping | `EffortlessMetrics/ripr-swarm` |
| `unsafe-review` | bug or weak `ReviewCard` / witness / comment-plan contract | `EffortlessMetrics/unsafe-review-swarm` |
| `tokmd` | bug or weak packet / manifest / context contract | `EffortlessMetrics/tokmd-swarm` |
| `cargo-allow` | bug or weak suppression/receipt schema; stale-entry enforcement | `EffortlessMetrics/cargo-allow` (no `-swarm` suffix; it is the policy tool itself) |
| `ast-grep` | rule false-positives / false-negatives in `tools/ub-rules/` | `EffortlessMetrics/ub-review` (local rules; upstream `ast-grep` engine bugs go to `ast-grep/ast-grep`) |
| `actionlint` | workflow-misclassification or weak rule | `rhysd/actionlint` |

## Upstream-issue contract

Every filed upstream issue must include:

- **Minimal repro** — the smallest input that reproduces the defect.
- **Command run** — the exact invocation.
- **Expected behavior** vs **actual behavior**.
- **Artifact excerpt** — the relevant slice of the sensor output (`badge-json`,
  `exposure-gaps.json`, `ReviewCard`, packet manifest, etc.).
- **Bun UB impact** (or broader product impact) — what review lane the defect
  affects.
- **Proposed acceptance criteria** — how the upstream fix will be verified.

## Local-workaround contract

When a local workaround is needed to keep `ub-review` usable (e.g. a dated
suppression receipt, a temporary threshold raise, a schema-conforming
suppression entry):

1. **Link the upstream issue** in the workaround's `reason` field (suppression
   entries, `policy/allow.toml` receipts, code comments).
2. **Make the workaround owned** — every workaround carries an `owner` and a
   review/expire date (suppressions: see the `non-rust-ripr-suppressions`
   receipt's dating obligation; threshold raises: see `#585`'s pattern).
3. **Revert when the upstream fix lands** — the workaround is temporary by
   contract. Suppression entries that reference resolved upstream issues
   should be removed in the same PR that pulls the upstream fix.

## Known-instability registers

Some sensor limitations are known and tracked upstream without blocking
`ub-review`:

| Limitation | Upstream issues | Local impact |
|---|---|---|
| ripr line-keyed `finding_id`s rot on code motion | `ripr-swarm#1053` | suppression entries may go dead after extraction; re-verify after edits |
| ripr cannot trace cross-module test paths (false-negatives) | `ripr-swarm#1054` | some predicate probes need suppression with the test name as oracle witness |
| ripr suppressions schema rejects `created`/`review_after` fields | (ripr 0.8.0) | per-entry dating carried by the `non-rust-ripr-suppressions` receipt instead (#586) |
| ripr `cfg(test)` skipping not implemented | `ripr-swarm#1055` | test-side probes need suppression twins |

These are not "filed and forgotten" — they are tracked here so local
workarounds cite them and revert when the upstream work lands.

## Cross-references (the other docs now point here)

- `docs/ARCHITECTURE.md` § "Trust boundary" → cross-references this spec.
- `docs/SENSOR_ROUTING.md` § "upstream" → cross-references this spec.
- `docs/CODEX_FINISH.md` § sensor gaps → cross-references this spec.
- `docs/RUNNER_IMAGE.md` § sensor defects → cross-references this spec.
- `docs/REQUIREMENTS.md` § sensor defects → cross-references this spec.
- `docs/specs/UB-REVIEW-SPEC-0005-sensor-tool-integration.md` § "Trust
  boundary" → cross-references this spec for the governance detail.

## Non-claims

This spec does **not** claim:

- **That upstream will fix the defect.** Filing upstream is the correct first
  step; the local workaround is the durable fallback.
- **That the routing table is exhaustive.** New sensors added to the
  `[tools]` registry should be added to the routing table here.
- **That suppression is a fix.** Suppression is a documented, dated, owned
  acknowledgment of a limitation — the upstream issue is the fix path.
