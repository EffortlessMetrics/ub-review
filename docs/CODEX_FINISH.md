# Codex lane goal

`ub-review` is the Bun UB review gate: one CI pass builds shared evidence,
many cheap model investigations reason over it, and the runner compiles one
grounded Pull Request Review. Do not build a generic review bot, Droid clone,
or agent harness. Build the evidence router and review compiler that makes
reviewers sharper per CI minute.

The end state is:

```text
GitHub runner
  -> checkout once
  -> diff once
  -> run fast sensors once
  -> build shared_context.md with receipts and bounded UB ledger context
  -> fan out MiniMax M3 lanes
  -> validate RIGHT-side inline candidates
  -> dedupe and refute
  -> post one grouped Pull Request Review
  -> upload the full artifact packet
```

Inline comments are capped, lane-prefixed, actionable, and evidence-backed.
The summary gives decision, confirmed findings, summary-only findings, failed
objections, residual risk, parked follow-ups, and missing evidence. No
LGTM/rubber-stamp language is allowed. Absence of sensor or model evidence is
never treated as safety.

## Technical direction

- Rust 2024 / Rust 1.95.
- Boring explicit code; no clever async unless it earns the complexity.
- `review-direct` is the default hot path.
- MiniMax M3 is the primary v0 provider.
- GLM is skipped for v0; wire it only so it can be enabled after approval.
- OpenCode Go / DeepSeek Flash may become direct provider candidate/refuter
  lanes after MiniMax works live.
- Codex/OpenCode/Pi-style agent harnesses are optional leased workers for
  investigation or patching, never the default hot-path orchestrator.
- Skip `cockpitctl` until a separate receipt director is actually needed.

Keep sensor claim boundaries explicit:

- `tokmd` produces deterministic PR packets and LLM-ready context.
- `ripr` is the PR-lane static mutation-exposure and repair-routing layer for Rust changed behavior; it does not run mutants or prove mutation outcomes.
- `unsafe-review` reports unsafe/native reviewability.
- `ast-grep` reports cheap structural route hits.

## Sensor issue rule

If `ub-review` exposes a real gap in a sensor, file it upstream instead of
papering over it locally:

| Sensor issue | File in |
|---|---|
| `ripr` bug or weak command/output contract | `ripr-swarm` |
| `unsafe-review` bug or weak ReviewCard/witness/comment-plan contract | `unsafe-review-swarm` |
| `tokmd` bug or weak packet/manifest/context contract | `tokmd-swarm` |

The issue should include minimal repro, command run, expected behavior, actual
behavior, artifact excerpt, impact on the Bun UB review lane, and proposed
acceptance criteria. Work around locally only when needed to keep `ub-review`
usable, and link the workaround to the upstream issue. Do not fork sensor
behavior silently.

## PR-by-PR workflow

Work in small green PRs. Each PR should move one layer toward the gate:

```text
baseline
MiniMax provider
provider fixtures
secure HTTP/timeouts
post smoke
dedupe
refuter
Bun integration
ledger context
deep mode
release binary
metrics
```

Do not mix layers. Use subagents deliberately for provider adapters, GitHub
posting, diff-line validation, prompt/schema work, tests, docs,
security/secret hygiene, and cleanup. Subagents may inspect and propose; the
main thread owns architecture, staging, and integration.

Clean up as you go. Remove throwaway worktrees, stale branches, temp dirs,
scratch packets, failed experiment files, and unnecessary Cargo target/cache
growth. Keep reproducible artifacts, tests, fixtures, and receipts.

Before finishing any PR, run:

```bash
cargo generate-lockfile
cargo fmt --all --check
cargo check --workspace --all-targets --locked
cargo test --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo doc --workspace --no-deps --locked
```

Then inspect git status, document what changed, note what remains, and push the
next smallest PR.

## Likely next polish

- If cargo installs are too slow on GitHub runners, replace
  `scripts/install-gh-runner-tools.sh` with release binary downloads for
  `tokmd`, `ripr`, and `unsafe-review`.
- If `ast-grep` npm install is noisy, disable it in `profiles/bun-ub-v0.toml`
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
