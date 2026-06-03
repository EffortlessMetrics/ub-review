# Unsafe review policy

`unsafe-review` is the third static evidence pillar in the Rust review stack.
It answers a PR-time reviewability question:

```text
Does this unsafe change have the safety contract, guard, test reach,
and witness route needed to make review credible?
```

It is advisory by default. It does **not** claim to prove unsafe Rust sound,
UB-free status, or Miri cleanliness. Runtime execution backstops remain the job
of focused tests, Miri, sanitizers, mutation testing, and downstream workload
receipts.

## Tool ownership boundaries

| Tool | Owns this question |
|---|---|
| `cargo-allow` | Is this exception owned, scoped, evidenced, and not silently broadened? |
| `ripr` | Does changed behavior appear exposed to a meaningful oracle? |
| `unsafe-review` | Does changed unsafe code have reviewable safety evidence? |
| `cargo-mutants` | Do tests fail against concrete mutants? |
| Miri | Does this concrete execution hit UB? |
| Codecov | Did this code execute? |

## Static unsafe-contract gaps

Unsafe code is reviewed on a separate rail from generic lint findings. The
expected static evidence classes are:

- safety contract for the unsafe operation;
- precondition guard at the boundary where bad inputs enter;
- layout and alignment witness for raw pointer, FFI, and representation seams;
- aliasing and lifetime evidence for borrowed/raw/shared ownership transitions;
- local test reach for the changed unsafe seam;
- witness route to Miri, sanitizer, runtime, mutation, or workload receipts
  where a concrete backstop is feasible.

`unsafe-review` can say that an unsafe seam is not currently reviewable; it
cannot say that the seam is sound.

## Repository commands

Rust repositories with an unsafe surface should expose these commands through
`xtask` or an equivalent command runner:

```bash
cargo xtask unsafe-review-pr
cargo xtask unsafe-review-repo
```

`unsafe-review-pr` should focus on changed unsafe/native seams. The full-repo
variant should audit retained gaps, stale suppressions, and witness-route drift.

## Artifact contract

The native unsafe-review artifact directory is:

```text
target/unsafe-review/
  cards.json
  pr-summary.md
  github-summary.md
  cards.sarif
  comment-plan.json
  witness-plan.md
  lsp.json
  receipt-audit.json
```

When orchestrated by `ub-review`, these receipts may also appear under the
run-specific sensor packet at `target/ub-review/sensors/unsafe-review/`.
Missing unsafe-review receipts must be reported as missing evidence, never as a
clean result.

## Policy receipts

Use `policy/allow.toml` for retained unsafe exceptions and durable
unsafe-review suppressions until real volume proves that a separate ledger would
improve reviewability. Suppression entries must be narrow, owned, evidenced, and
time-bounded. A suppression is not a substitute for a witness route when a
high-risk unsafe seam can reasonably be exercised.

## CI posture

- PRs: run advisory `unsafe-review first-pr` / `unsafe-review-pr`, upload cards,
  summary, and SARIF, and do not block by default.
- Risk PRs: require a witness route for changed high-risk unsafe seams.
- Nightly: run Miri, sanitizer, and targeted unsafe witness jobs where feasible.
- Release: require no unexplained new unsafe-review gaps on public or
  load-bearing crates.

## cargo-allow composition

`cargo-allow` owns durable unsafe exceptions. `unsafe-review` owns whether the
unsafe seam is reviewable. Runtime tools provide concrete witness receipts. A
retained exception should cite all three layers when available:

```toml
[[exception]]
id = "allow-unsafe-0042"
kind = "unsafe"
path = "crates/foo/src/raw.rs"
owner = "ffi"
classification = "ffi_boundary"
reason = "Raw pointer conversion required at C ABI boundary."
evidence = [
  "doc:docs/safety/foo-ffi.md",
  "unsafe-review:target/unsafe-review/cards.json",
  "miri:target/miri/foo_ffi_receipt.json",
  "test:ffi_rejects_unaligned_pointer",
]
review_after = "2026-09-01"
```
