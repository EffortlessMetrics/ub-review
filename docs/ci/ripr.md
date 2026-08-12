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
local `receipt.json`. Use `--base <rev>` when the PR targets another base and
`--out-dir <path>` only when a separate receipt directory is useful.

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
tool issue where appropriate, or an owned exact suppression receipt.
