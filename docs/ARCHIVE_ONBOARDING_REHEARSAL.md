# Preliminary archive onboarding rehearsal (#1340)

Owner: ub-review/tests. The Linux x86_64 Rust integration target
`tests/archive_onboarding.rs` executes the resolver shell extracted from the
actual `action.yml`, in strict `install-mode=release`, then invokes only its
archive-extracted executable. It never uses Cargo's integration-test binary as
the reviewed CLI. An inert curl adapter serves frozen archive/checksum bytes
and the current enable command's release metadata; unknown URLs fail. This
adapter is a transport fixture, not a published release or replacement resolver.

Prepare inputs separately from execution. Building preliminary bytes may use
Rust 1.95.0; installing UB Review and generating setup must not. The child
process PATH contains a fixed system-tool allowlist and recording tripwires
instead of real cargo/rustc/rustup/gh/workspace executables. Commands receive a
cleared environment, no credentials and file-backed logs with 30-second waits.
Python 3 with PyYAML is a declared harness prerequisite for independent YAML
parsing. The harness timeout kills/reaps its direct child, not a general process
tree containment guarantee.

Supply a frozen directory containing:

- `ub-review-x86_64-unknown-linux-gnu.tar.gz`: one root `ub-review` executable.
- `ub-review-x86_64-unknown-linux-gnu.tar.gz.sha256`: the archive SHA256 receipt.
- `manifest.json`: phase `preliminary_local_archive`, target
  `x86_64-unknown-linux-gnu`, source_commit, source_tree, toolchain, version,
  tag (`v` plus the exact binary version), binary_sha256, archive_sha256,
  checksum_sha256. Record build profile, preparation commands, byte lengths
  and environment in the retained preparation packet as well.

Run explicitly (the receipt directory must be new and absolute):

```console
UB_ARCHIVE_PACKET=/absolute/frozen-packet \
UB_ARCHIVE_RECEIPTS=/absolute/new-receipts \
  cargo test --locked --test archive_onboarding frozen_archive_onboarding -- --ignored --nocapture
```

Cargo builds/launches the Rust harness here; it is not available to child
installation/setup commands. Ordinary workspace tests report this archive
campaign as ignored, never as an executed acceptance pass.

The campaign freezes a conventional Rust workspace and a mixed Rust/JavaScript/C
repository with native-risk source, existing CI and unrelated-file sentinels.
Repository names and audit output paths contain spaces and shell metacharacters.
It tests manual `init --no-guide` and rolls it back before using the separate
`enable --mode advisory --inspect` setup route. `--inspect` writes workflow and
configuration. Both init/config conflict and enable replay refusal run twice,
with byte/mode/path inventories compared. Tokenless `audit-ci` and local-only
`setup-ci --print-pr` execute; no `--open-pr`, provider or post command is allowed.
Rollback deletes only the exact generated path set, preserves original Git/CI/
source bytes and a file added after setup, and records before/after inventories.

Negative controls run the production release resolver twice with unavailable
assets and require failure with no outputs or source fallback. Production auto
mode can choose PATH; the harness must reject that result. Replacing installed
bytes must fail the frozen executable digest guard before CLI execution.

The versioned receipt records actual commands/exits, exact frozen input,
harness/Action digests and limits; logs and generated samples survive success or
failure. `bounded_pass` covers only the enumerated first-slice scenarios.
A failed/missing/omitted scenario is never a pass.

Current source produces an advisory MiniMax workflow with provider/token inputs,
`posting: review` and a release-tag Action ref. The campaign retains that actual
sample and **does not execute it**. It does not claim the sample is automatically
model-off, artifact-only, immutable-SHA pinned or proven fork-safe. The separate
`docs/ADOPTION_ADVISORY.md` recipe supplies supported model-off/artifact-only
inputs and an explicitly reviewed full Action SHA for a contained evaluation.
These generated-default/pinning boundaries stay visible for the existing owners;
this test lane changes no production generator.

Final #816 candidate acceptance remains pending after selected source/docs and
installer fixes are merged and its exact archive exists. A final run requires
fresh identities and a separately reviewed candidate-phase manifest contract;
this first target deliberately accepts only preliminary inputs. Public-release
mode (#811 after #817), previous-supported-binary upgrade/rollback, the full leaf
regression matrix, workflow execution, live provider/delivery and platform
portability remain unproven. A merged harness cannot close #1340 or #816.
No release/tag/package, credentials, branch protection, Required gate, default,
pin, workflow or remote publication mutation is part of this lane.
