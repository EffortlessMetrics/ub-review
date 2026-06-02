# Codex finish notes

This package is intentionally close to usable but not overfit.

## First thing to verify

Run CI with Rust 1.95:

```bash
cargo generate-lockfile
cargo fmt --all --check
cargo check --workspace --all-targets --locked
cargo test --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo doc --workspace --no-deps --locked
```

## Likely next polish

- If cargo installs are too slow on GitHub runners, replace
  `scripts/install-gh-runner-tools.sh` with release binary downloads for
  `tokmd`, `ripr`, and `unsafe-review`.
- If `ast-grep` npm install is noisy, disable it in `configs/bun-gh-runner.toml`
  until a pinned binary path is available.
- Consider adding a `--changed-files-from` mode later for PR event payloads, but
  keep git diff as the default because it is reproducible locally.
- Improve model provider adapters once MiniMax and OpenCode Go response
  envelopes are proven in live Bun runs. GLM is skipped for v0.

## Non-goals

- No issue-comment spam or one-comment-per-lane posting.
- No synthetic verdicts from missing evidence.
- No blocking policy gate.
- No cockpitctl layer yet.
- No heavy witnesses by default.
