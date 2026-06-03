# Source exception ledger

`cargo-allow` is the preferred ledger layer for source-tree exceptions in Rust
repositories that adopt the `ub-review` style. It should own the durable record
for syntax-visible exceptions, while `xtask` and CI remain the control plane
that runs checks and verifies the cited evidence.

This ledger complements the external UB campaign ledger described in
[UB_LEDGER.md](UB_LEDGER.md). The external ledger is read-only campaign memory
for `ub-review` runs. The source exception ledger lives in the reviewed
repository and records individual source exceptions with owners, reasons,
evidence, and review dates.

## Responsibility split

```text
cargo-allow = source exception ledger
xtask       = repo-specific control plane and gate orchestration
ripr        = static mutation-exposure analysis
mutation    = runtime test-oracle backstop
Codecov     = execution-surface receipt
Clippy/rustc = compiler and lint floor
```

`cargo-allow` should answer whether a visible exception exists, whether it has a
stable receipt, who owns it, why it remains allowed, what evidence supports it,
and when it needs review. It should not be treated as a compiler, Clippy, build,
proc-macro expansion, `ripr`, mutation, coverage, `unsafe-review`,
`cargo-deny`, or `cargo-vet` replacement. Those tools produce evidence that the
ledger can reference.

## Source-tree scope

Use `cargo-allow` for durable receipts around:

- `unsafe` blocks and declarations;
- `unwrap`, `expect`, `panic!`, and related panic families;
- `#[allow]` and `#[expect]` suppressions;
- indexing and slicing exceptions that are syntax-visible;
- generated-file exceptions;
- tracked scripts, workflows, and non-Rust source exceptions;
- PR exception diffs and agent worklists.

Keep bespoke `xtask` checks only when the repository needs semantics that the
ledger does not claim, such as exact counted panic baselines, release readiness,
coverage policy, mutation execution, supply-chain policy, or richer
repo-specific lint gates.

## Ledger file

The standard location is:

```text
policy/allow.toml
```

A receipt should be owned, explain the invariant, link evidence, and include a
review date or expiry. Example shape:

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

## Wrapped commands

Developers should not need to remember raw ledger commands. Repositories should
wrap them through `xtask`:

```bash
cargo xtask allow-audit
cargo xtask allow-check
cargo xtask allow-diff
cargo xtask allow-worklist
```

Typical wrappers invoke:

```bash
cargo-allow audit
cargo-allow check --mode no-new
cargo-allow diff --base origin/main
cargo-allow worklist --format json
```

## CI receipts

Use a stable artifact layout so PR reviews, agents, and release gates can find
ledger output without repo-specific discovery logic:

```text
target/cargo-allow/
  pr-summary.md
  check.md
  check.receipt.json
  worklist.json
```

The default PR signal is a markdown diff receipt:

```bash
cargo-allow diff \
  --base origin/main \
  --format markdown \
  --output target/cargo-allow/pr-summary.md
```

The main or release gate should preserve a check receipt:

```bash
cargo-allow check \
  --mode no-new \
  --format markdown \
  --receipt target/cargo-allow/check.receipt.json \
  --output target/cargo-allow/check.md
```

## Review rule

The repository style should prefer one source exception ledger where possible:

```text
No invisible source exceptions.
No anonymous broad allows.
No retained panic/unsafe/script/generated surface without owner, reason,
evidence, and review date.
No PR broadening without a diff receipt.
```

`ub-review` should consume these receipts as evidence when available, but it
must still report missing or stale evidence as missing evidence rather than as a
clean review result.
