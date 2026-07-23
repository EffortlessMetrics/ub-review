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
`cargo-allow` defaults to `0.1.8`; `ripr` defaults to `0.10.1`;
`unsafe-review` defaults to `0.3.4`; `actionlint` defaults to `v1.7.12`:

```bash
export UB_REVIEW_TOKMD_VERSION="1.12.0"
export UB_REVIEW_CARGO_ALLOW_VERSION="0.1.8"
export UB_REVIEW_RIPR_VERSION="0.10.1"
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
