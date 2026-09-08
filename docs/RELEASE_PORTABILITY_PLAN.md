# PR #1265: immutable v0.1.0 portability proof

Status: design for the Rust replacement. Exact-head validation and dispatched
execution receipts are recorded in PR #1265; the prior runs below retain their
historical source identity.

## Authority and retained evidence

- [Issue #1071](https://github.com/EffortlessMetrics/ub-review/issues/1071)
  defines the clean-container portability obligation.
- [The September 5 repair direction](https://github.com/EffortlessMetrics/ub-review/pull/1265#issuecomment-5549425901)
  requires Rust-owned validation, independent release identity, explicit CI
  authorization, discriminating tests, and a fresh exact-head run.
- `policy/ci-budget.toml`, `policy/ci-risk-packs.toml`, and
  `policy/ci-lanes.toml` govern cost and proof routing.
- Run `33166458366` and its runtime/resolver artifacts are historical evidence
  from the superseded shell harness at `b1198e2`. They establish the observed
  v0.1.0 Ubuntu 24.04/glibc 2.39 success and Ubuntu 22.04/glibc 2.35 loader
  rejection. They do not validate the replacement harness.
- The v0.1.0 initializer's empty provider/impact defaults are immutable
  historical behavior, fixed separately on main by #845. Preserve the exact
  two expected policy failures and the separate explicit-config passing packet.

## Bounded design

Keep `xtask/src/main.rs` limited to the new command's dispatch/help. Put the
implementation in `xtask/src/release_portability/` with separate metadata,
archive, runtime, packet, and receipt responsibilities. Remove the large shell
harness; do not replace it with embedded script authority.

The host command resolves live GitHub release/tag/asset metadata before Docker
execution. Follow annotated tags to their commit with a bounded, cycle-checked
chain. Compare the observed tag commit, exact asset names, IDs, sizes, HTTPS
URLs, and API digests against the immutable v0.1.0 policy. Independently download
and verify both the archive and checksum bytes. Retain resolved metadata and
observed comparisons, rather than copying constants into claimed observations.

Use typed serde receipts, bounded decompression/archive inspection, and a
single validation path for both real and negative-control inputs. Reject
missing/duplicate assets, moved tags, digest/size drift, traversal, links,
duplicate/wrong-root archive members, and wrong product/version identity.

Run two explicitly named disposable Ubuntu containers. Only the verified
release executable and a minimal synthetic review fixture enter the runtime
containers; no host Rust tooling, repository source, credentials, or source
fallback enters either container. Invoke explicit OS commands from Rust and
perform assertions, JSON/NDJSON parsing, and receipt construction in Rust on the
host. Retain OS/architecture/libc/kernel observations, version/help, initializer,
doctor, both model-off packets, checksums, inventory, and all negative controls.

The workflow is `workflow_dispatch` only. Existing independent-baseline checks
cover ordinary PR unit proof. The dispatched jobs record their event, actor,
run ID, workflow SHA, and checked-out source SHA. A separate clean resolver
job exercises the local Action's strict missing-asset failure and invokes the
already-built xtask executable to validate and write its receipt; it installs
neither Cargo nor rustc. Workflow YAML carries orchestration, not receipt JSON.

## Acceptance and proof

- Typed accept/reject tests distinguish metadata parsing, annotated tag
  dereference/cycles/movement, exact asset selection, duplicates/omissions,
  checksum/size drift, hostile archives, wrong version, missing or conflicting
  matrix rows, and deterministic receipts.
- Ubuntu 24.04/x86_64/glibc 2.39 must run the exact release and produce a valid
  explicit-config model-off packet; Ubuntu 22.04/glibc 2.35 must reject it at
  the loader specifically on `GLIBC_2.39`.
- Both runtime environments and the resolver control must prove absence of
  Cargo, rustc, and source fallback. The two runtime containers also contain no
  UB Review source checkout.
- Run `cargo fmt --all -- --check`, focused xtask tests, locked workspace
  all-target/all-feature check, full locked workspace tests, strict locked
  Clippy, `cargo run --locked -p xtask -- policy-check`, actionlint, and
  `git diff --check`. Coordinate the shared Cargo proof slot before building.
- Keep #1265 open until a newly authorized exact-head portability dispatch and
  normal gate are green. Respond to the three review threads with concrete
  repair evidence, and resolve them only after the exact-head checks.

## Boundaries, rollback, and cleanup

No release, tag, asset, resolver default, branch protection, or public support
policy changes. No support extrapolation to other distributions/architectures,
and no current-product claim from immutable v0.1.0 behavior. Live metadata plus
the downloaded digest identifies the tested existing asset; this is not a new
signature, provenance, or release authorization claim.

The output directory must be new so stale receipts cannot satisfy a new run.
Retain failed-run diagnostics without a successful matrix. Remove only
containers and scratch artifacts created by this command. Keep the workflow
manual if reverted; historical receipts remain historical. Handle unrelated
release/install documentation drift in a separate builder-ready follow-up.
