# Repository bootstrap and first live use

The initial repository scaffold has landed in:

```text
https://github.com/EffortlessMetrics/ub-review
```

Current development should happen through small PRs against `main`. Do not treat
`origin/main` as an empty bootstrap target.
For non-Bun Rust repos adopting the same operating baseline, use
[PORTING_BASELINE.md](PORTING_BASELINE.md).

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
The Bun gate should use a verified commit SHA:

```yaml
uses: EffortlessMetrics/ub-review@804d198b5a15a0df94bb4f43750dba71165916cd
```

The current known-good pin is `804d198b5a15a0df94bb4f43750dba71165916cd`.
Move it only after a draft Bun UB PR proves:

- one grouped Pull Request Review is posted, or `github-review-skip.json` plus
  `post-result.json` proves an artifact-only skip;
- `target/ub-review/running-summary.md` exists;
- `target/ub-review/review/review.md` exists;
- `target/ub-review/review/github-review.json` exists only when reviewer-value
  content survives compilation;
- `target/ub-review/input/diff.patch` exists;
- sensor receipts exist under `target/ub-review/sensors/*/ub-review-sensor-status.json`;
- missing sensors or model lanes are explicit missing evidence, not clean results.

After downloading the artifact, run the packet verifier:

```bash
python scripts/verify-bun-review-artifacts.py target/ub-review \
  --min-ok-model-lanes 5 \
  --require-no-model-evidence-failures
```

Do not float the Bun workflow on `main`. Tags are for release rollouts; the
daily Bun hunt should stay on the latest verified SHA.
