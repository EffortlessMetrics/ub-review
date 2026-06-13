# UB-REVIEW-SPEC-0010 - release binary and Action install surface

Status: authored 2026-06-06 (release surface spec wave, docs-only).
Child of UB-REVIEW-SPEC-0001. Documents how ub-review gets onto a runner or
a dev box today; intent is marked as intent. Maturity, stated precisely: the
install machinery is production through the source-build path (the live Bun
pin is a commit SHA, which always builds from source), and the release-asset
fast path is implemented but unexercised - the repository has published zero
GitHub releases as of this writing. Tags `v0` and `v0.1` predate the release
workflow (added in PR #90, .github/workflows/release-binary.yml) and never
triggered it; no `v*` tag has been pushed since. The umbrella maturity row
therefore marks this surface partial: the fallback is the proven path, while
the release binary remains the unpublished fast path.

## Purpose

The install surface answers adoption friction, not review quality. A consumer
repo should get a working `ub-review` binary with one `uses:` line and no
toolchain ceremony; a runner-image maintainer should be able to bake the
sensors and verify the bake; a developer should be able to build from a
checkout with stock cargo. Every path ends at the same binary contract: the
later steps of the composite action (`run`, `post`, `gate-check`) consume
whatever `steps.runner.outputs.bin` resolved, with no path-specific behavior
differences (action.yml "Resolve ub-review runner").

## User question

```text
How do I install this without rebuilding the world?
```

Honest current answer: on a tagged action ref you would skip the build, but
no release asset exists yet, so today every fresh runner builds ub-review
from the action source (a `cargo build --locked --release` of this crate).
The world being rebuilt is bounded - one binary crate, cacheable via
`~/.cargo` and `CARGO_TARGET_DIR` (README "Bootstrap note") - and the
machinery to skip it is in place, waiting on the first published release.

## Lifecycle moment

Before any review work:

- consumer workflow job setup, on every `pull_request` pass that invokes the
  action;
- runner-image build time, for repos that bake a standard image
  (docs/RUNNER_IMAGE.md);
- local development setup (README "Local development";
  scripts/smoke-local.sh).

## Consumer

- a consumer repo workflow author pasting `uses: EffortlessMetrics/ub-review@<ref>`;
- a runner-image maintainer running scripts/install-review-image-tools.sh
  and `ub-review doctor --require-core-tools`;
- a local developer building from a checkout;
- release engineering on this repo pushing `v*` tags.

## Inputs

### GitHub Action usage

```yaml
- uses: EffortlessMetrics/ub-review@<full-commit-sha or v* tag>
  with:
    install-mode: auto      # default; auto | release | source | path
    setup-rust: 'true'      # default; rustup install 1.95.0 --profile minimal
    install-tools: 'true'   # default; best-effort advisory sensor install
    tool-bundle: core       # default; none | core | bun-fast | full
```

Install-mode resolution, exactly as implemented (action.yml "Resolve
ub-review runner"):

- unknown `install-mode` values are a hard error, not a downgrade;
- `path` requires an executable `binary-path` or errors;
- `auto` uses an existing `ub-review` on PATH first; otherwise, if the
  action ref looks like a release tag (`v` plus a digit), it tries the
  release download; a non-tag ref (the commit-SHA pin) gets a notice and
  goes straight to the source build;
- `release` tries the download first and falls back to the source build on
  any download/checksum/extraction failure, with a workflow warning; the one
  hard-error path is a malformed `release-version` or `release-asset`
  input, which fails the job in `validate_release_request` before any URL
  is constructed;
- `source` is the deterministic fallback: copy the action source into
  `RUNNER_TEMP`, `cargo generate-lockfile`, then
  `cargo build --locked --release`, honoring a caller-set
  `CARGO_TARGET_DIR`.

Release request constraints (action.yml `validate_release_request`,
`download_release_binary`): `release-version` defaults to the action ref
with any `refs/tags/` prefix stripped and must use only letters, numbers,
dot, underscore, or dash; `release-asset` (default
`ub-review-x86_64-unknown-linux-gnu.tar.gz`) must be a bare file name - no
path separators, no `..` - in the same charset. The download is attempted
only on Linux x86_64 runners (`uname` check); `.tar.gz`/`.tgz` assets are
downloaded with a sibling `<asset>.sha256` receipt, whose first field must be a
64-hex SHA-256 digest matching the archive before extraction. They are then
extracted and must contain an executable named `ub-review`, while any other
asset name is treated as the raw binary. Every failure branch returns to the
source build.

Rust toolchain setup (action.yml "Select Rust toolchain"): when
`setup-rust` is true and rustup exists, it installs and defaults 1.95.0
(`rust-toolchain.toml` pins the same). When rustup is missing but cargo
exists, the step warns and continues with the existing toolchain -
non-fatal by design. It errors only when neither rustup nor cargo exists.

### Required secrets mapping (inputs to env)

The action forwards inputs to the binary as `UB_REVIEW_*` env vars; the
binary never reads raw provider secrets from anywhere else (action.yml run
step; src/main.rs `model_api_key_env`):

```text
minimax-api-key    -> UB_REVIEW_MINIMAX_API_KEY
minimax-api-url    -> UB_REVIEW_MINIMAX_API_URL
opencode-api-key   -> UB_REVIEW_OPENCODE_API_KEY
opencode-api-url   -> UB_REVIEW_OPENCODE_API_URL
github-token       -> --github-token flag on run (PR-thread seeding) and
                      UB_REVIEW_GITHUB_TOKEN on the post step
(plus UB_REVIEW_PROFILE, UB_REVIEW_PRESET, UB_REVIEW_GITHUB_EVENT_ACTION
 on the run step; UB_REVIEW_FAIL_ON_GATE, UB_REVIEW_MODE,
 UB_REVIEW_GATE_OUTCOME_PATH on the gate-check step)
```

No secret is required to produce a packet; missing model keys are recorded
as missing evidence (docs/GH_RUNNER_SETUP.md). `FACTORY_API_KEY` is
deliberately not an input and doctor does not check it
(docs/RUNNER_IMAGE.md).

### Sensor install bundles

`install-tools: true` runs scripts/install-gh-runner-tools.sh with
`UB_REVIEW_TOOL_BUNDLE` set from `tool-bundle`:

- `none` installs nothing;
- `core`, `bun-fast`, and `full` all install the same six advisory sensors:
  tokmd (pinned 1.12.0, override `UB_REVIEW_TOKMD_VERSION`), cargo-allow,
  ripr, unsafe-review (all three unpinned `cargo install`), ast-grep (npm,
  unpinned), actionlint (go install, pinned v1.7.12, override
  `UB_REVIEW_ACTIONLINT_VERSION`); `full` adds only a notice that optional
  sensors (semgrep, gitleaks, osv-scanner, cargo-audit, cargo-deny,
  cppcheck, zizmor) must be preinstalled and enabled explicitly;
- an unknown bundle value fails the install step with an `::error::`
  annotation naming the accepted values, matching the strict
  `install-mode` validation - a typo'd bundle can no longer silently
  install a different sensor set than the workflow asked for.

Each sensor install is individually best-effort: a failed install warns and
the sensor is recorded as skipped at review time, never as clean evidence
(scripts/install-gh-runner-tools.sh header comment).

### Runner image path (self-hosted / standard image)

docs/RUNNER_IMAGE.md and docs/GH_RUNNER_SETUP.md define the baked-image
contract: scripts/install-review-image-tools.sh installs the five Rust core
sensors with `cargo install --locked --root "$UB_REVIEW_TOOL_DIR"` and
actionlint with `GOBIN="$UB_REVIEW_TOOL_DIR/bin" go install` (pinned
v1.7.12) (default prefix `/opt/ub-review`); the image puts `$UB_REVIEW_TOOL_DIR/bin` on PATH
and must not point `UB_REVIEW_TOOL_DIR` at the `bin` directory itself.
`UB_REVIEW_TOOL_DIR` is consumed only by the install script and docs - the
binary finds tools on PATH. `UB_REVIEW_CACHE_DIR` is consumed by the binary
as the cache root (src/cli.rs DoctorArgs/CacheWarmArgs env attrs; src/main.rs
`cache_root_path`, default `.cache/ub-review`). `UB_REVIEW_STANDARD_IMAGE=true`
turns doctor's core-tool checks from advisory into blocking (below).

### cargo install path

There is no crates.io publication and no documented `cargo install
ub-review` from a registry. The supported source path is a checkout:
`cargo build --locked --release` (action.yml `build_from_source`; README
"Bootstrap note") or equivalently `cargo install --path .`; the action's
source fallback runs the identical build. Rust 1.95, edition 2024 (rust-toolchain.toml,
Cargo.toml). Registry publication is unstated intent, not a plan.

## Output artifact / user surface

- The resolved binary: `steps.runner.outputs.bin`, plus `release-url`/
  `release-dir` when the download succeeded or `source-dir` when the source
  build ran (action.yml). Workflow log annotations state which path was
  taken and why a fallback happened.
- Release assets, once a `v*` tag is pushed
  (.github/workflows/release-binary.yml): `ub-review-x86_64-unknown-linux-gnu.tar.gz`
  containing the single `ub-review` executable, plus a sibling `.sha256`
  receipt, uploaded to the GitHub release (created with `--verify-tag`) and
  duplicated as a 14-day workflow artifact. Tag names must match
  `^v[0-9][A-Za-z0-9._-]*$` or the workflow errors.
- `ub-review doctor` stdout (src/main.rs `cmd_doctor`): profile name, box
  summary, limits, cache root, binary path, install status (on PATH, shadowed,
  or explicit-path fix), profile hash, base cache hit/miss, one line per
  configured tool (found/missing, `--version` output, rule-cache hit/miss),
  and one line per provider showing the env var name and `present`/`missing` -
  values are never printed.
- `ub-review cache warm` artifacts (src/main.rs `cmd_cache_warm`):
  `<cache-root>/bases/<base-tree-sha>/manifest.json`, per-tool
  `bases/<base-tree-sha>/<tool>/manifest.json` and
  `rules/<tool>/manifest.json`, and `<cache-root>/latest-manifest.json`.
  The manifest path is keyed by base tree SHA (`git_tree_sha`); the profile
  hash (`profile_config_hash`) and each core tool's command and observed
  version are recorded inside the manifest but are not compared when doctor
  reports a base-cache hit - a hit means only that some prior warm wrote a
  manifest for that tree. Consumers wanting profile/toolset-exact hits must
  compare the manifest fields themselves. Consuming workflows must track
  the cache key themselves; `latest-manifest.json` is the pointer.

## Required fields

```text
release asset            tar.gz containing executable `ub-review`; sibling
                         `<asset>.sha256` receipt (release-binary.yml)
runner step outputs      bin (always); release-url|release-dir or
                         source-dir depending on path (action.yml)
cache warm manifest      schema_version 1; profile; profile_hash; base;
                         base_tree_sha; cache_root; base_cache_dir;
                         rules_cache_dir; tools[] with tool, command,
                         status (found|missing), version, rule_cache_dir,
                         base_cache_dir (src/main.rs CacheWarmManifest)
doctor provider lines    env var name + present|missing, never the value
doctor pins              CORE_REVIEW_TOOLS = tokmd, cargo-allow, ripr,
                         unsafe-review, ast-grep, actionlint;
                         STANDARD_IMAGE_TOKMD_VERSION = 1.12.0
                         (src/main.rs)
```

## Advisory vs blocking behavior

- Installing ub-review itself is blocking for the job: if the chosen mode
  and the source fallback both fail, the job fails. There is no "run
  without the binary" state.
- `setup-rust` is advisory when a toolchain already exists (warning on
  missing rustup); blocking only when no cargo exists at all (action.yml).
- Sensor installs are advisory always: failures warn, and the missing
  sensor surfaces as missing evidence in the packet, never as a job
  failure and never as clean evidence.
- `doctor` is advisory by default. It becomes blocking only with
  `--require-core-tools` or `UB_REVIEW_STANDARD_IMAGE=true`
  (src/main.rs `cmd_doctor`): it then bails when any of the six core tools
  is missing, or when a pinned tool's `--version` output does not contain
  the pinned version token (`command_version_matches` tolerates `v`
  prefixes and punctuation splits). Today only tokmd has a pin
  (`expected_standard_image_tool_version` returns Some only for tokmd).
- `cache warm` never blocks; tools missing at warm time are recorded as
  `status: missing` in the manifest.

## Fail-closed behavior

- Release download failures fail closed into the source build, never into
  "no binary": every download, checksum, and extraction error branch of
  `download_release_binary` returns to `build_from_source` - the
  input-validation branches inside it (`validate_release_request`)
  hard-fail the job instead, by design - and the extracted candidate must
  be an executable named `ub-review` before it is accepted (action.yml).
- Release request inputs are validated before any URL is constructed: bare
  file names only, restricted charset, no traversal (action.yml
  `validate_release_request`); the release workflow refuses malformed tags
  (release-binary.yml "Validate release tag").
- Doctor reports provider key presence without printing values; under
  standard-image enforcement, missing core tools and tokmd version drift
  are hard failures, not warnings (src/main.rs `cmd_doctor`;
  docs/RUNNER_IMAGE.md "Policy").
- Honest gaps, where the surface does not fail closed today:
  - cargo-allow and ast-grep are NOT version-pinned: the install scripts
    take latest and doctor's drift check does not cover them. Since #335
    the pins cover tokmd (1.12.0), ripr (0.8.0), and unsafe-review (0.3.3)
    in both the install script and doctor — unpinned-ripr drift is how
    #316 stayed invisible until a local 0.5/0.8 mismatch surfaced it.
    The foreign-dialect cargo-allow path from #318 now skips with a linked
    reason instead of producing a schema-red failure. #319 is covered by the
    tokmd run preflight: the sensor receipt names installed vs pinned
    versions before `--preset bun-ub` commands run.
  - on the dev-side install surface, `cargo xtask precommit` records
    missing sensors as `success: true` skipped receipts with exit 0,
    indistinguishable from relevance skips (#320), and the receipts do not
    say how to install the missing tool (#321).

## Trust boundary / non-claims

```text
the install surface delivers a binary; it proves nothing about the repo
release assets are integrity-receipted (.sha256) and checked by the action
  before use, but not signature-verified
pinning by commit SHA pins the source you build; pinning by tag trusts
  GitHub release storage for the prebuilt asset
missing sensors after install are missing evidence, never clean evidence
doctor verifies presence and three version pins (tokmd, ripr,
  unsafe-review since #335); tokmd run preflight guards #319 and
  cargo-allow planning guards #318, but doctor still does not certify sensor
  output contracts
```

Version pinning doctrine: consumers pin the action by full commit SHA. The
current known-good pin is
`EffortlessMetrics/ub-review@804d198b5a15a0df94bb4f43750dba71165916cd`,
validated by EffortlessSteven/bun#49 (README "Copy/paste Bun setup";
docs/GH_RUNNER_SETUP.md). Pin advance protocol: do not float on `main`;
update the SHA only after this repo's artifact verifier passes and the Bun
consumer workflow succeeds at the candidate SHA.

Support matrix, stated as proven rather than promised: ubuntu GitHub-hosted
runners are the production environment (the Bun consumer, this repo's own
gate, and the release workflow all run ubuntu); the release asset targets
Linux x86_64 only, and the action's download path refuses other platforms
and falls back to source. Windows is exercised as a local development
environment by this repo's own development, not claimed as a supported CI
target. macOS is unexercised.

The six reliance questions:

```text
Rely on:     one uses: line yields a working binary or a failed job, never
             a silent half-install; SHA pins source-build the exact pinned
             code whenever a build happens - under install-mode=auto a
             pre-existing ub-review on PATH wins over the pin, use
             install-mode=source to force the pinned build; the source
             fallback is deterministic and --locked; doctor never prints
             secret values.
Break gate:  nothing in this surface feeds the gate verdict. Install
             failures fail the job before any gate evaluation; sensor
             install failures become missing evidence, which blocks only
             under intelligent-ci required-sensor policy (spec 0003).
Advisory:    sensor installs, setup-rust on an existing toolchain, doctor
             without --require-core-tools, cache warm.
PR-visible:  nothing. Install activity is workflow-log annotations only.
Artifact:    runner step outputs, cache warm manifests, doctor stdout,
             release assets and .sha256 receipts (once published).
Ten minutes: paste the README workflow with the pinned SHA; the first run
             rustups 1.95.0, builds ub-review from the action source
             (cacheable), best-effort installs six sensors, and produces a
             packet. No secrets needed for the packet itself.
```

## Validation commands

```bash
ub-review doctor --profile gh-runner --base origin/main --require-core-tools
                                        # presence, tokmd pin, provider env,
                                        # cache hit/miss; bails on image drift
UB_REVIEW_TOOL_BUNDLE=core bash scripts/install-gh-runner-tools.sh
                                        # idempotent sensor install
ub-review cache warm --profile gh-runner --base origin/main
                                        # writes base+rules manifests
cargo generate-lockfile && cargo build --locked --release
                                        # the exact action source fallback
scripts/smoke-local.sh                  # doctor -> init -> plan -> dry-run
actionlint .github/workflows/release-binary.yml
```

Packaging validation can run without publishing by manually dispatching
`.github/workflows/release-binary.yml`; it builds the Linux x64 archive,
writes the sibling `.sha256`, and uploads both as workflow artifacts. Full
release-path validation still requires pushing a `v*` tag and observing a
consumer run with `install-mode: release` (first slice below).

## Implementation PR slices

This spec is docs-only; it routes open work:

1. Publish the first real release: push a `v*` tag, confirm
   release-binary.yml publishes the asset plus `.sha256`, and prove
   `install-mode: release` and tagged `auto` end to end on a consumer run.
   Until this lands, the release path is implemented-but-unexercised and
   release notes must not claim a prebuilt install. No issue yet.
2. DONE: Verify the `.sha256` receipt in the action's download path before
   accepting the asset.
2a. DONE: Add a `workflow_dispatch` dry-run to release-binary.yml so
   maintainers can prove archive + `.sha256` packaging before release
   authorization; GitHub release creation/upload remains tag-push-only.
3. DONE: unknown `tool-bundle` values fail the install step with an error
   naming the accepted values, matching the strict `install-mode`
   validation.
4. PARTIALLY DONE (#335): ripr (0.8.0) and unsafe-review (0.3.3) are
   pinned in the install script and doctor
   (`expected_standard_image_tool_version`); the ripr gate-decision
   receipt landed under spec 0005. Remaining: pin cargo-allow itself;
   #318 covered only the foreign-ledger skip path.
5. Make xtask precommit missing-tool receipts honest and actionable:
   distinguish missing-tool skips from relevance skips (#320) and include
   install instructions in the receipt (#321).

## Release note claim

```text
ub-review installs with one Action line: commit-SHA pins build the exact
pinned source with a locked toolchain, sensors install best-effort with
every gap recorded as missing evidence, and doctor verifies the runner
image without printing a single secret value.
```

The claim "prebuilt release binaries skip the build on tagged refs" is
machinery-true but unproven; it may not appear in release notes until
slice 1 publishes and exercises the first asset.
