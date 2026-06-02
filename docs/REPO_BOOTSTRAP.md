# Repository bootstrap and first live use

The initial repository scaffold has landed in:

```text
https://github.com/EffortlessMetrics/ub-review
```

Current development should happen through small PRs against `main`. Do not treat
`origin/main` as an empty bootstrap target.

## Local verification

Before opening a PR, run:

```bash
cargo generate-lockfile
cargo fmt --all --check
cargo check --workspace --all-targets --locked
cargo test --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo doc --workspace --no-deps --locked
```

## First Bun fork verification

In the Bun fork, add:

```text
.github/workflows/ub-review-packet.yml
```

using `examples/bun/.github/workflows/ub-review-packet.yml` from this repository.
The first run should call:

```yaml
uses: EffortlessMetrics/ub-review@main
```

Use `@main` until a draft Bun UB PR proves:

- one grouped Pull Request Review is posted or a `post-error.json` receipt explains why not;
- `target/ub-review/running-summary.md` exists;
- `target/ub-review/review/review.md` exists;
- `target/ub-review/review/github-review.json` exists;
- `target/ub-review/input/diff.patch` exists;
- sensor receipts exist under `target/ub-review/sensors/*/ub-review-sensor-status.json`;
- missing sensors or model lanes are explicit missing evidence, not clean results.

After downloading the artifact, run the packet verifier:

```bash
python scripts/verify-bun-review-artifacts.py target/ub-review \
  --min-ok-model-lanes 10 \
  --require-no-model-evidence-failures
```

After that run posts a useful review and uploads a complete packet, tag `v0` in
this repository and switch the Bun workflow to:

```yaml
uses: EffortlessMetrics/ub-review@v0
```
