# ub-review $TAG

A same-model MiniMax review team for GitHub PRs: specialist lanes review the PR,
a proof-planning lane selects relevant PR-specific tests, a deterministic broker
runs them safely, and the reporter distills one review plus a CI gate result.

## Install (one command)

```bash
ub-review enable --mode gate --model minimax --action-sha <this-tag-sha>
```

Then add `MINIMAX_API_KEY` as a required repository secret and, optionally,
`OPENCODE` for provider fallback before opening a PR. MiniMax remains operational
without the optional fallback secret. See
[docs/QUICKSTART.md](docs/QUICKSTART.md) for the 5-minute guide.

## Modes

| `--mode` | Behavior |
|---|---|
| `advisory` | Reviews and comments; never blocks. |
| `gate` | Reviews + deterministic-floor gate enforcement. Recommended. |
| `strict` | `gate` + reporter verdict can block (review-forward). |

## Artifacts

- `ub-review-x86_64-unknown-linux-gnu.tar.gz` — the binary
- `.sha256` — checksum (verified on download by the action)

The GitHub Action downloads and verifies this binary by default
(`install-mode: auto`); source-build is the fallback only.

## Calibration

Every run emits `review/calibration.json`. Use `ub-review status`,
`ub-review recommend`, and `ub-review promote` to turn calibration data into
actionable mode guidance. See [docs/ADOPTION_MODES.md](docs/ADOPTION_MODES.md).
