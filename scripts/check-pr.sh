#!/usr/bin/env bash
set -euo pipefail
cargo generate-lockfile
cargo fmt --all --check
cargo check --workspace --all-targets --locked
cargo test --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo doc --workspace --no-deps --locked
# Advisory supply-chain check (non-blocking; skip if cargo-audit is absent).
cargo xtask audit || true
