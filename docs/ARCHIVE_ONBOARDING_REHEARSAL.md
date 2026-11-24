# Archive onboarding rehearsal (#1340)

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
The automatic timeout control waits for a Unix-socket readiness handshake,
then exercises the same runner timeout arm with a 50 ms deadline. It checks
SIGKILL, absence of the direct child's `/proc` entry before test-side cleanup,
and a physically persisted failed/timed-out/reaped receipt. The Python fixture
has no descendants and self-exits on a bounded socket timeout if kill is omitted;
negative controls still perform test-side kill/reap after inspecting the result.
This is runner-oracle coverage, not a production process-containment repair.

## Preliminary local archive

Supply a frozen directory containing:

- `ub-review-x86_64-unknown-linux-gnu.tar.gz`: one root `ub-review` executable.
- `ub-review-x86_64-unknown-linux-gnu.tar.gz.sha256`: the archive SHA256 receipt.
- `manifest.json`: phase `preliminary_local_archive`, target
  `x86_64-unknown-linux-gnu`, source_commit, source_tree, toolchain, version,
  tag (`v` plus the exact binary version), binary_sha256, archive_sha256,
  checksum_sha256. Record build profile, preparation commands, byte lengths
  and environment in the retained preparation packet as well.
  Source commit/tree/toolchain are preliminary manifest labels, not verified
  cryptographic binary provenance. Archive/checksum/executable digests and the
  executable's version are checked separately; final #816 must bind provenance.

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
Harness-owned cleanup (not a production rollback command) deletes only the exact generated path set, preserves original Git/CI/
source bytes and a file added after setup, and records before/after inventories.

Negative controls run the production release resolver twice with unavailable
assets and mismatched checksum bytes and require failure with no outputs or source fallback. Production auto
mode can choose PATH; the harness must reject that result. A same-byte workspace path is also rejected. Replacing installed
bytes must fail the frozen executable digest guard before CLI execution.

The versioned receipt records actual attempted commands, spawn failures/timeouts and exits, exact frozen input,
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

## Selected candidate intake

Retain both original Actions ZIP downloads and a separate custody JSON file.
Populate the custody file from the independently observed producer run, commit,
tree, package version and artifact metadata. Record the raw receipt SHA-256
independently before handoff. The harness uses this file as its trust input.
Keep it outside the ZIPs and retain it read-only.

| JSON key | Required fields |
| --- | --- |
| `producer` | `repository`, `head_sha`, `tree_sha`, `workflow`, `producer_ref`, `run_id`, `run_attempt`, `artifact_id`, `artifact_name`, `retained_until` |
| `bytes_artifact`, `packet_artifact` | `id`, `name`, `run_id`, `run_attempt`, `retained_until`, `length`, `sha256` for each ZIP |
| `package_version`, `raw_packet_sha256` | Version from the producer commit and digest of the original `authorized-release-packet.json` bytes |

Use numeric IDs, lengths in bytes, retention times in Unix seconds and lowercase
hex digests. The bytes artifact holds the fixed release archive, checksum and
release notes. The packet artifact holds the original producer receipt. The
selected candidate path additionally requires GNU `timeout` and `unzip`. Its
automatic synthetic controls also use `zip` and GNU `tar`.

```console
UB_CANDIDATE_BYTES_ZIP=/absolute/candidate.zip \
UB_CANDIDATE_PACKET_ZIP=/absolute/packet.zip \
UB_CANDIDATE_FACTS=/absolute/independent-custody.json \
UB_ARCHIVE_RECEIPTS=/absolute/new-receipts \
  cargo test --locked --test archive_onboarding selected_candidate_archive_onboarding -- --ignored --nocapture
```

Before the resolver starts, intake verifies both ZIPs and the raw packet,
checks producer identity and retention, verifies the archive and checksum, and
derives the single executable member's digest. The existing resolver and
onboarding campaign then run against private copies of those admitted bytes.
Every campaign CLI call checks the installed path and digest. Synthetic controls
exercise the same intake and resolver and label their receipts
`synthetic_fixture`. Set `UB_CANDIDATE_PROVENANCE=synthetic_fixture` when
running the full entry point with substitute artifacts.

The #816 release qualification still requires the selected producer's retained
bytes and a real onboarding run. #1071 can use the same candidate intake but
retains separate ELF, pinned Ubuntu consumer, and model-off execution and packet
evidence. Public release mode (#811 after #817), previous-supported-binary
upgrade and rollback, the full leaf regression matrix, generated workflow
execution and live provider delivery require separate proof.
