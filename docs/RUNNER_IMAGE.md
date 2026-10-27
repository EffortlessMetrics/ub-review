# Runner Image

The Bun UB gate should not discover its core sensors at review time. On the
standard image, these binaries are part of the review optical bench:

- `tokmd`
- `cargo-allow`
- `ripr`
- `unsafe-review`
- `ast-grep`
- `actionlint`

Install them during image build:

```bash
scripts/install-review-image-tools.sh
```

The script installs Rust and npm-backed tools into `$UB_REVIEW_TOOL_DIR/bin`,
where `UB_REVIEW_TOOL_DIR` defaults to `/opt/ub-review`. It uses
`cargo install --locked --root "$UB_REVIEW_TOOL_DIR"` for Rust tools and
`go install` for `actionlint`, so the standard image build must provide Go.
`tokmd` defaults to version `1.12.0` because the Bun profile depends on the
current on-diff `bun-ub`, `cockpit`, and `context` command surfaces.
`cargo-allow` defaults to `0.1.8`; `ripr` defaults to `0.10.0`;
`unsafe-review` defaults to `0.3.4`; `actionlint` defaults to `v1.7.12`:

```bash
export UB_REVIEW_TOKMD_VERSION="1.12.0"
export UB_REVIEW_CARGO_ALLOW_VERSION="0.1.8"
export UB_REVIEW_RIPR_VERSION="0.10.0"
export UB_REVIEW_UNSAFE_REVIEW_VERSION="0.3.4"
export UB_REVIEW_ACTIONLINT_VERSION="v1.7.12"
scripts/install-review-image-tools.sh
```

Set the image environment:

```bash
export PATH="/opt/ub-review/bin:$PATH"
export UB_REVIEW_TOOL_DIR="/opt/ub-review"
export UB_REVIEW_CACHE_DIR="/var/cache/ub-review"
export UB_REVIEW_STANDARD_IMAGE="true"
```

`UB_REVIEW_TOOL_DIR` is the install prefix. Put its `bin` directory on `PATH`;
do not set `UB_REVIEW_TOOL_DIR` to the `bin` directory itself.

`UB_REVIEW_STANDARD_IMAGE=true` makes `ub-review doctor` fail if any core
sensor is missing or if any pinned core tool (`tokmd`, `cargo-allow`, `ripr`,
`unsafe-review`, or `actionlint`) drifts from its expected version. On generic
GitHub-hosted runners, missing tools remain missing evidence unless
`install-tools=true` installs them successfully.

## Diagnostic version probes

`doctor` and `cache warm` share a diagnostic adapter (#1557). Each `--version`
probe has a two-second total budget: up to 1.5 seconds for direct-child completion
and EOF on **both** pipes, with the remaining time reserved for kill/reap. This
budget is independent of the sensor's `timeout_sec`. An exited leader with a
descendant holding a pipe is an incomplete, timed-out probe.

Capture retains at most 4 KiB per stream in memory, reads in 1 KiB chunks and
writes no stream files. Exceeding either ceiling cancels collection and triggers
direct-child cleanup. Closing cancelled pipe readers requires no reader joins.
OS spawn/setup calls and OS scheduling cannot provide a hard real-time guarantee;
all asynchronous collection and cleanup waits use the same absolute deadline.

Doctor rows include `probe=` (`missing`, `setup-failed`, `spawn-failed`,
`read-failed`, `wait-failed`, `nonzero`, `timeout`, `output-limit`, `unknown` or
`valid`) and `cleanup=`. Cleanup distinguishes `not-started`, `reaped`,
`check-failed-reaped`, `kill-failed`, `reap-failed` and `reap-timeout`; failed cleanup never claims reaping.
Remaining provider/fix diagnostics continue. Required core tools still fail on
missing or unavailable versions; advisory doctor still succeeds with fixes.

Only successful, complete, within-budget output can supply a version. The first
nonblank stdout line (stderr when stdout is empty) is projected to numeric dotted
version tokens, with the configured tool ID, within 160 bytes. Arbitrary child
prose and terminal/workflow directives are discarded. Blank, undecodable or
unrepresentable output is `unknown`. This changes the old arbitrary first-line
display/cache text to a constrained representation; it does not change the pin
matcher or establish executable identity. Tool-specific grammar and bundle
selection remain owned by #1067, including its known compatibility-token limits.

Cache rule/base manifests and tool receipts add `probe_status` and `probe_cleanup`.
Every failed/incomplete probe stores `version: null`; the existing receipt status
remains `missing` for unavailable versions and `found` for available versions.
A later successful warm can replace the unavailable identity.

The adapter owns and terminates/reaps only the direct child. It does **not**
contain arbitrary descendant trees. #1279/#1269 retain general stream/packet and
process-tree ownership. Linux held-pipe controls use a harness subreaper to kill
and reap their own descendant on success and failure; this is not a product
containment claim. Cross-platform async process pipes come from Tokio, scoped to
this diagnostic call; no sensor/proof runner or general scheduler is migrated.

## Cache Layers

The cache has three layers:

```text
/var/cache/ub-review/
  rules/
    tokmd/
    cargo-allow/
    ripr/
    unsafe-review/
    ast-grep/
    actionlint/
  bases/
    <base-tree-sha>/
      manifest.json
      tokmd/
      cargo-allow/
      ripr/
      unsafe-review/
      ast-grep/
      actionlint/
```

The current scaffold records cache manifests and creates the per-tool cache
directories. Sensors still own their internal cache formats; `ub-review` does
not fake indexes for tools that do not expose one.

## tokmd Receipts

The Bun profile uses `tokmd` as three receipts, not one whole-repo dump:

```text
sensors/tokmd/analyze.md
sensors/tokmd/analyze.json
sensors/tokmd/cockpit.md
sensors/tokmd/cockpit.json
sensors/tokmd/context.md
```

The intended commands are:

```bash
tokmd analyze \
  --preset bun-ub \
  --effort-base-ref "$BASE_REF" \
  --effort-head-ref "$HEAD_REF" \
  <existing changed paths>

tokmd cockpit \
  --base "$BASE_REF" \
  --head "$HEAD_REF"

tokmd context \
  --budget 64000 \
  --output sensors/tokmd/context.md \
  <existing changed paths>
```

`analyze` is the primary on-diff evidence packet. `cockpit` is the compact PR
overview for summaries and metrics. `context` is the token-budget receipt over
changed files only, including charged tokens, full tokens, and inclusion policy.

Warm the cache for a base tree:

```bash
ub-review cache warm \
  --profile gh-runner \
  --base origin/main \
  --out /var/cache/ub-review
```

Inspect the image and cache:

```bash
ub-review doctor \
  --profile gh-runner \
  --base origin/main \
  --require-core-tools
```

Doctor reports:

- tool presence;
- tool `--version` output;
- provider key env presence for `UB_REVIEW_MINIMAX_API_KEY` and
  `UB_REVIEW_OPENCODE_API_KEY`, without printing values;
- cache root;
- profile/config hash;
- base cache hit or miss;
- rule cache hit or miss.

Doctor does not check `FACTORY_API_KEY`; the current action has no Factory
provider input. The artifact verifier still treats raw Factory key assignments
as secret leaks.

## Policy

For the Bun profile:

- missing `tokmd`, `cargo-allow`, `ripr`, `unsafe-review`, `ast-grep`, or
  `actionlint` on the standard image is image drift and should fail `doctor`;
- `tokmd` reporting a version other than `1.12.0` on the standard image is image
  drift and should fail `doctor`;
- missing tools on a generic hosted runner are missing evidence, not proof of a
  clean review;
- sensor defects should be filed in the matching `*-swarm` repo, not hidden in
  local glue (canonical contract: `docs/specs/UB-REVIEW-SPEC-0016-sensor-upstream-boundary.md`).
