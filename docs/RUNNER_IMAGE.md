# Runner Image

The Bun UB gate should not discover its core sensors at review time. On the
standard image, these binaries are part of the review optical bench:

- `tokmd`
- `ripr`
- `unsafe-review`
- `ast-grep`

Install them during image build:

```bash
scripts/install-review-image-tools.sh
```

The script installs with `cargo install --locked --root "$UB_REVIEW_TOOL_DIR"`
where `UB_REVIEW_TOOL_DIR` defaults to `/opt/ub-review`. `tokmd` defaults to
version `1.11.1` because the Bun profile depends on the current on-diff
`analyze`, `cockpit`, and `context` command surfaces:

```bash
export UB_REVIEW_TOKMD_VERSION="1.11.1"
scripts/install-review-image-tools.sh
```

Set the image environment:

```bash
export PATH="/opt/ub-review/bin:$PATH"
export UB_REVIEW_TOOL_DIR="/opt/ub-review/bin"
export UB_REVIEW_CACHE_DIR="/var/cache/ub-review"
export UB_REVIEW_STANDARD_IMAGE="true"
```

`UB_REVIEW_STANDARD_IMAGE=true` makes `ub-review doctor` fail if any core
sensor is missing. On generic GitHub-hosted runners, missing tools remain
missing evidence unless `install-tools=true` installs them successfully.

## Cache Layers

The cache has three layers:

```text
/var/cache/ub-review/
  rules/
    tokmd/
    ripr/
    unsafe-review/
    ast-grep/
  bases/
    <base-tree-sha>/
      manifest.json
      tokmd/
      ripr/
      unsafe-review/
      ast-grep/
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
  --preset estimate \
  --effort-base-ref "$BASE_REF" \
  --effort-head-ref "$HEAD_REF"

tokmd cockpit \
  --base "$BASE_REF" \
  --head "$HEAD_REF"

tokmd context \
  --budget 64k \
  --mode bundle \
  --output sensors/tokmd/context.md \
  <existing changed paths>
```

`analyze` is the primary on-diff evidence packet. `cockpit` is the compact PR
overview for summaries and metrics. `context` is bounded lane context over
changed files only.

The desired future `tokmd analyze --preset bun-ub` command should replace the
`estimate` preset once `tokmd` exposes that preset. Until then, the runner uses
the verified effort-delta command rather than shipping a known-failing sensor
invocation. Upstream tracker: `EffortlessMetrics/tokmd-swarm#182`.

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
- cache root;
- profile/config hash;
- base cache hit or miss;
- rule cache hit or miss.

## Policy

For the Bun profile:

- missing `tokmd`, `ripr`, `unsafe-review`, or `ast-grep` on the standard image
  is image drift and should fail `doctor`;
- missing tools on a generic hosted runner are missing evidence, not proof of a
  clean review;
- sensor defects should be filed in the matching `*-swarm` repo, not hidden in
  local glue.
