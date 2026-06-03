# Cargo-allow ledger layer

`cargo-allow` is the default source-tree exception ledger for Rust repositories
that follow this review style. It does not replace `xtask`, Clippy, no-panic
scanners, `ripr`, mutation testing, Codecov, release gates, `cargo-deny`,
`cargo-vet`, or unsafe-review tooling. It owns the durable source exception
ledger; the other tools produce or validate evidence.

The expected governance flow is:

```text
source-tree exception
  -> owned receipt
  -> evidence links
  -> expiry or review date
  -> PR diff report
  -> agent worklist
```

## Responsibilities

`cargo-allow` should answer source-exception questions without executing project
code:

- what source-visible exceptions exist;
- why each exception is allowed;
- who owns each exception;
- what evidence supports each exception;
- when each exception expires or needs review;
- whether a pull request broadened, narrowed, or improved the exception set.

Use it for syntax-visible exception inventory and receipts, including:

| Repo-style need | `cargo-allow` role |
| --- | --- |
| `unsafe` receipts | Ledger and diff reporting |
| `unwrap`, `expect`, and `panic!` receipts | Source exception inventory |
| `#[allow]` and `#[expect]` governance | Owned suppression receipts |
| indexing and slicing exceptions | Syntax-visible exception receipts |
| generated files | Allowlist and evidence |
| scripts, workflows, and non-Rust tracked files | Source-tree exception ledger |
| PR review signal | `cargo-allow diff --base origin/main` |
| agent task queue | `cargo-allow worklist --format json` |
| source-exception explanations | `cargo-allow explain <allow-id>` |

Keep semantic proof outside `cargo-allow`. It should not compile, run Cargo
metadata, run rustc, run Clippy, run build scripts, expand proc macros, run
`ripr`, run unsafe-review, or prove tests are adequate.

## Standard split

The repo template should prefer one shared source exception ledger instead of
hand-rolled ledgers for every exception class:

```text
cargo-allow = source exception ledger
xtask       = repo control plane
ripr        = static mutation-exposure analysis
mutation    = runtime backstop
Codecov     = execution-surface receipt
Clippy/rustc = compiler and lint floor
```

`cargo-allow` owns:

```text
policy/allow.toml
source exception inventory
exception ownership
evidence links
review_after / expires metadata
PR diff summaries
agent worklists
```

`xtask` owns:

```text
orchestration
repo-specific gates
CI planning and local evidence maps
release readiness
calling cargo-allow
aggregating receipts
```

The rule of thumb is:

```text
cargo-allow says: "This visible exception exists, is owned, and has evidence."
other tools say: "The evidence is real."
```

## Policy ledger shape

Repositories should make `policy/allow.toml` the primary source exception
ledger. A typical receipt looks like this:

```toml
[[allow]]
id = "allow-0042"
kind = "panic"
family = "indexing_slicing"
path = "crates/parser/src/span.rs"
owner = "parser"
classification = "validated_span_invariant"
reason = "Parser validates TextRange before slicing."
created = "2026-06-01"
review_after = "2026-09-01"
evidence = [
  "doc:docs/safety/parser-spans.md",
  "test:parser_rejects_invalid_text_range",
  "ripr:target/ripr/span-gap.json",
]

[allow.selector]
ast_kind = "index_expr"
container = "slice_checked_text_range"
```

Each receipt should have a stable id, kind, owner, classification, reason,
creation date, review or expiry date, evidence references, and a selector that
is specific enough to avoid broad anonymous exceptions.

## Xtask wrappers

Developers should not need to remember raw command details. Repositories should
wrap the common flows with `xtask` commands:

```bash
cargo xtask allow-audit
cargo xtask allow-check
cargo xtask allow-diff
cargo xtask allow-worklist
```

Those wrappers should call the corresponding `cargo-allow` operations:

```bash
cargo-allow audit
cargo-allow check --mode no-new
cargo-allow diff --base origin/main
cargo-allow worklist --format json
```

## CI artifacts

Use a stable artifact convention so humans, agents, and release gates can find
receipts consistently:

```text
target/cargo-allow/
  pr-summary.md
  check.md
  check.receipt.json
  worklist.json
```

The default pull-request gate should emit a markdown diff receipt:

```bash
cargo-allow diff \
  --base origin/main \
  --format markdown \
  --output target/cargo-allow/pr-summary.md
```

Main and release gates should fail on unapproved broadening and preserve both
human-readable and machine-readable receipts:

```bash
cargo-allow check \
  --mode no-new \
  --format markdown \
  --receipt target/cargo-allow/check.receipt.json \
  --output target/cargo-allow/check.md
```

## What it consolidates

Prefer `cargo-allow` over bespoke ledgers for:

- non-Rust allowlists;
- generated-file exception ledgers;
- executable and script exception ledgers;
- source-level panic, unwrap, and expect exception ledgers;
- allow and expect suppression ledgers;
- broad exception worklists;
- PR exception diff summaries.

Keep richer repo-specific checks when they need semantics `cargo-allow` does not
claim, such as exact panic-family baselines, release-readiness reports, mutation
analysis, coverage, dependency supply-chain policy, or unsafe semantic review.

## Review invariant

The standard Rust repo template should enforce these invariants:

```text
No invisible source exceptions.
No anonymous broad allows.
No retained panic, unsafe, script, or generated surface without owner, reason,
evidence, and review date.
No PR broadening without a diff receipt.
```
