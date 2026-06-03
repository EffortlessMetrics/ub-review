# ripr PR receipts

`ripr` is static mutation-exposure analysis for Rust/Cargo workspaces.

It catches much of the same weak-test/oracle signal that mutation testing
catches, but earlier and cheaper. It does not run mutants, report
`killed`/`survived` outcomes, prove correctness, or replace runtime mutation
testing.

Use `ripr` in the PR lane. Use mutation testing as targeted, nightly, or release
runtime confirmation.

## Repo-style stack

```text
cargo-allow = durable source-exception ledger
ripr = static mutation-exposure / repair-routing layer
xtask = repo control plane and receipt aggregator
cargo-mutants = runtime backstop
Codecov = execution-surface receipt
```

`cargo-allow` answers what exceptions exist, who owns them, why they are allowed,
and when they expire. `ripr` answers whether behavior changed in the diff and
whether the current test surface appears to expose that behavior to a meaningful
oracle.

## Standard PR packet

Repos that use this stack should expose a single control-plane command such as:

```bash
cargo xtask ripr-pr
```

The command should run the repository's pinned `ripr` first-PR path, typically:

```bash
ripr first-pr --root . --base origin/main --head HEAD
```

The expected packet is:

```text
target/ripr/pr/
  pr-summary.md
  repo-exposure.json
  review.md
  agent-packet.json
  first-useful-action.md
  first-useful-action.json
```

When `ub-review` runs `ripr`, it preserves the normal stdout/stderr/status
receipts under `target/ub-review/sensors/ripr/` and mirrors any
`target/ripr/pr/` packet into `target/ub-review/sensors/ripr/pr/` so the review
artifact contains the static exposure packet.

## Repair loop

Treat `ripr` as a repair router, not only as an advisory note:

```text
one changed behavior
→ one repairable gap
→ one focused test or output proof
→ one before/after receipt
```

Agentic repair instructions should start from the `ripr first-pr` packet, repair
one named exposure gap, add the focused proof, run the receipt command, and stop
when the before/after receipt moves. Do not broaden the work into a generic
"improve tests" task.

## Mutation routing

Use `ripr` to choose when the expensive runtime backstop is warranted:

```text
Default PR:
  fmt
  check
  clippy
  tests
  cargo-allow diff
  ripr static mutation-exposure packet

Risk PR:
  targeted cargo-mutants slice when ripr finds high-risk exposure gaps
  or when touched paths are high-risk

Nightly:
  broader mutation matrix

Release:
  mutation/readiness must be clean enough to ship
```
