#!/usr/bin/env bash
set -euo pipefail
cargo run -- doctor --profile gh-runner
cargo run -- init --profile gh-runner --force --path target/ub-review-smoke.toml
cargo run -- plan --config target/ub-review-smoke.toml --profile gh-runner --base HEAD --head HEAD --write --out target/ub-review-smoke
cargo run -- run --config target/ub-review-smoke.toml --profile gh-runner --base HEAD --head HEAD --dry-run --out target/ub-review-smoke
