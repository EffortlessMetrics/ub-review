# ripr

`ripr` is static mutation-exposure analysis. It catches the same class of
findings mutation testing catches - weak test/oracle exposure - but earlier
and cheaper because it is static and PR-time.

Mutation testing remains the slower runtime backstop for findings static
analysis cannot predict. `ripr` shifts mutation signal left.

Treat `ripr` as an economical source of mutation-style signal, not as a second
parallel proof obligation that every PR must duplicate with full runtime
mutation. Use runtime mutation on main, nightly, release, or explicitly labeled
high-risk PRs where it buys signal.

## Local ready-mode feedback

Run the repository adapter before pushing Rust changes:

```text
cargo xtask ripr
```

The command resolves the merge base of `origin/main` and `HEAD`, compares the
current tracked working tree to that revision, writes the exact diff to
`target/xtask/ripr/diff.patch`, and invokes the pinned
`ripr 0.10.0` with `--mode ready` for the raw badge decision, JSON detail, and
tool-native human feedback. It preserves those verbatim outputs as
`gate-decision.json`, `exposure-gaps.json`, and `feedback.txt`, plus a bounded
local `receipt.json`. Use `--base <rev>` when the PR targets another base. The
output path is deliberately fixed under `target/xtask/ripr/`; the command does
not accept arbitrary cleanup or overwrite targets.

Clean diffs and diffs without Rust inputs are explicit `skipped` outcomes.
Untracked Rust files are rejected because Git cannot include them in the diff;
add or stage them before rerunning. A missing or wrong-version tool fails with
the exact pinned install command:

```text
cargo install ripr --locked --version 0.10.0 --force
```

This command is a fast local preview, not hosted authority. A missing merge
base is a hard error; it never falls back to a two-dot comparison. GitHub still
evaluates a merge ref that can differ from a local merge-base/working-tree
snapshot, so the
hosted gate and its artifact remain authoritative. The adapter does not change
the strict-zero threshold, classify findings, edit suppressions, or replace
`ripr`; address each reported finding with a discriminating test, an upstream
tool issue where appropriate, or an owned exact suppression receipt. A nonzero
local count is advisory and does not change the command's exit status; only the
repository's existing hosted gate policy owns the strict-zero decision.

RIPR 0.10.0 detail findings do not carry suppression state. The adapter
therefore reconciles the total canonical-gap count while treating the badge's
suppressed/unsuppressed partition as tool-reported rather than independently
derived. The production packet makes the same boundary explicit:
`sensors/ripr/exposure-gaps.json` uses
`ub-review.ripr_exposure_gaps.v3`, declares `raw_pre_policy`, and omits
per-entry suppression and threshold fields. Its complete raw finding and
canonical gap totals reconcile with the pinned badge envelope; its stable IDs
are raw diagnostic identities, not a per-ID join to the aggregate-only badge.
The exact RIPR stdout and stderr are preserved beside the projection as
`exposure-gaps.ripr.stdout` and `exposure-gaps.ripr.stderr`; use those files
when a full tool-native record is required. The
badge at `sensors/ripr/gate-decision.json` remains the sole strict-zero and
suppression-partition authority (#873).

Schema migration is fail-closed: the verifier accepts v3 only and rejects
v2 artifacts because their capped entries cannot establish complete
per-finding coverage. Re-run the producer on the exact hosted head to migrate;
there is no safe mechanical conversion from a truncated v2 receipt.

## Hosted retrieval

For a specific `ub-review/gate` run, download the named workflow artifact
without relying on the PR comment or aggregate check summary:

```text
gh run view <run-id> --json headSha --jq .headSha
# Compare that SHA with the reviewed commit before continuing.
gh run download <run-id> --name ub-review-gate --dir target/hosted-ripr
```

The downloaded tree contains
`sensors/ripr/exposure-gaps.json` (complete stable-ID projection),
`sensors/ripr/exposure-gaps.ripr.stdout` (exact RIPR JSON stdout), and
`sensors/ripr/exposure-gaps.ripr.stderr` (exact RIPR stderr). Verify the
artifact with `python scripts/verify-bun-review-artifacts.py target/hosted-ripr`
using the same review-profile and repository-kind arguments recorded by the
run. Reject the artifact if the run `headSha` does not equal the reviewed
commit; the verifier validates the receipt schema and complete raw-ID join but
cannot infer which commit a manually selected download represented.
