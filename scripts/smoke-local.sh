#!/usr/bin/env bash
set -euo pipefail

# A real diff. `--base HEAD --head HEAD` produces zero changed files, so no
# lane or sensor ever materializes and the smoke run passes without exercising
# anything — including the `init` output it just wrote.
OUT=target/ub-review-smoke
CONFIG=target/ub-review-smoke.toml

if [[ -n "${UB_REVIEW_SMOKE_BASE:-}" ]]; then
  BASE="$(cargo run --locked --package xtask -- smoke-base --base "$UB_REVIEW_SMOKE_BASE")"
else
  BASE="$(cargo run --locked --package xtask -- smoke-base)"
fi

cargo run --locked -- doctor --profile gh-runner
cargo run --locked -- init --profile gh-runner --force --path "$CONFIG"
cargo run --locked -- plan --config "$CONFIG" --profile gh-runner --base "$BASE" --head HEAD --write --out "$OUT"
cargo run --locked -- run --config "$CONFIG" --profile gh-runner --base "$BASE" --head HEAD --dry-run --out "$OUT"
cargo run --locked -- gate-check --gate-outcome "$OUT/review/gate_outcome.json" --fail-on-gate true
