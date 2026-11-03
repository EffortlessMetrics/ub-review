# Convergence session learnings (2026-10-30 to 2026-11-02)

## Session summary

One 3-day convergence session processed the entire open PR backlog: ~76 merged, ~25 closed (superseded or rejected), 5 issues filed, 1 red-main bug found and fixed (#1608), 1 security advisory resolved (#1631). The session ended with only 1 open PR (#1384, perma-held diagnostic) for the first time in the repo history.

## What was built

### The fix-or-suppress train

A repeatable autonomous cycle: rebase onto current main, resolve conflicts by taking main's side, fire the gate, diagnose failures from gate artifacts, append ripr receipts, re-fire, merge with state=MERGED readback. Documented in rebase_one.sh + append_receipts.py + train_one.sh + queue_drafts.sh.

### The tooling

- `rebase_one.sh` — automated rebase with conflict resolution (suppressions.toml, allow.toml, test-mod collisions, Cargo.lock) and gate dispatch
- `append_receipts.py` — deduped ripr receipt appending from gate artifacts
- `train_one.sh` — full per-PR cycle: rebase → gate → receipt → re-gate → ready → merge
- `queue_drafts.sh` — sequential queue processing over all open drafts
- `union_allow.py` — allow.toml receipt-log union resolver with ours-fallback

## The 8 systemic learnings

### 1. The gate tests branch heads, not merge refs

Two gate-green PRs merged sequentially can combine into a red main. #1520 changed normalize_repo_path; b9df50b added a broker test pinning the old behavior. Each was green; the combination broke request_attribution_reuses_only_the_executed_bun_target. Fixed by #1608.

**Lesson:** the gate validates a snapshot, not a composition. After any merge burst, run the previously-failing tests on current main.

### 2. Receipt commits shed on every re-rebase

Receipt commits carrying only ledger additions become empty during rebase and are dropped. Every re-rebase sheds them again.

**Lesson:** consolidate receipts into the feature commit before the first rebase. See #1626.

### 3. Concurrent sessions need ownership boundaries

Two sessions working the same branch caused an accidental force-push (#1373) that overwrote the concurrent session's commits.

**Lesson:** check lastPush before touching any branch. Force-with-lease failure = back off.

### 4. Cross-process test coordination requires OS-level locks

RwLock coordinates threads within one process. cargo test runs test binaries as separate processes. Binary A's fixture writes and binary B's spawns are uncoordinated.

**Lesson:** the fixture coordination needs a cross-process lock (POSIX flock) or the O_TMPFILE pattern. See #1625.

### 5. GitHub mergeability is eventually consistent

mergeable_state DIRTY persisted across empty commits, re-rebases, and gate successes for #1396.

**Lesson:** wait 5+ min after pushing. Empty commits force recompute. If merge fails with 0 real conflicts, push an empty commit and wait again.

### 6. --delete-branch cascades on stacked PRs

Merging a stacked PR with --delete-branch deletes the shared base branch, auto-closing dependents. #1306's merge deleted fix/1305-terminal-work-queue, auto-closing #1314.

**Lesson:** check baseRefName before merging. If not main, the PR is stacked.

### 7. allow.toml reason fields are append-only review logs

Conflicts between two append-only logs resolve as: keep main's version + append branch-side Reviewed segments that main lacks.

**Lesson:** union_allow.py handles most cases but falls back to main's side for non-log conflicts (like #1481).

### 8. Path separator mismatches cause Windows test failures

Tests asserting error messages with file paths use forward slashes; Windows produces backslashes. Partially fixed by #1630.

**Lesson:** normalize paths before comparison, or use Path::join with separate components in assertions.

## Issue inventory

| Issue | Title | Status |
|---|---|---|
| #1625 | Flaky fixture test (CLOEXEC race) | Open — fix designed, needs implementation |
| #1626 | Receipt shedding in rebase_one.sh | Open — fix designed, needs implementation |
| #1627 | Canonical determinism design | Open — design options documented |
| #1628 | Windows test failures | Open — 3 of 5 fixed, 2 remain |
| #1629 | anyhow security advisory | Closed — resolved by #1631 |

## PR disposition

| Disposition | Count |
|---|---|
| Merged | ~76 |
| Closed as superseded | 6 (#1455, #1564, #1525, #1548, #1481, #1495) |
| Closed as rejected | 15+ |
| Perma-held (open) | 1 (#1384) |
| Owner-deferred (closed) | 3 (#1408, #1396, #1379) |
