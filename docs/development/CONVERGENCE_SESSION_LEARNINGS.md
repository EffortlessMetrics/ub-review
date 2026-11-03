# Convergence session learnings (2026-10-30 to 2026-11-02)

## What happened

One 3-day convergence session processed the entire open PR backlog: ~76 merged, ~25 closed (superseded or rejected), 5 issues filed, 1 red-main bug found and fixed (#1608), 1 security advisory resolved (#1631). The session ended with 0 open non-perma-held PRs for the first time in the repo history.

## The fix-or-suppress train: what worked

The repeatable cycle (rebase onto current main, resolve conflicts by taking mains side, fire the gate, diagnose failures from gate artifacts, append ripr receipts, re-fire, merge with state=MERGED readback) converged 30+ author-held drafts. The cycle is documented in rebase_one.sh + append_receipts.py + train_one.sh + queue_drafts.sh.

### What made it work

1. **Conflict resolution rules per file family.** Each file type has a known resolution:
   - .ripr/suppressions.toml: mains ledger + branch-unique additions
   - policy/allow.toml: append-only reason logs, unioned by Reviewed segments
   - Test-mod collisions: close HEADs fn, start the branchs fn
   - Cargo.lock: take either side (both are valid)

2. **Gate artifacts as the diagnostic oracle.** Every gate failure was diagnosed by downloading the runs artifacts and reading the blocking reasons. No guessing.

3. **Receipt IDs are content-addressed.** They follow code, not line numbers. This means they survive rebases when hunks apply cleanly.

4. **Landing readbacks as the convergence signal.** state=MERGED is the only trustworthy evidence that a PR landed.

### What the train could NOT do

1. **Multi-file feature-ports.** Branches predating the module extraction need someone who understands the feature.
2. **Design decisions.** #1408s canonical-determinism issue requires a design choice.
3. **GitHub infrastructure bugs.** #1396s mergeability bug is outside the repos control.

## Systemic learnings

### 1. The gate tests branch heads, not merge refs

Two gate-green PRs merged sequentially can combine into a red main. This happened: #1520 changed normalize_repo_path; b9df50b added a broker test pinning the old behavior. Each was green; the combination broke request_attribution_reuses_only_the_executed_bun_target.

**Implication:** after any burst of merges, run the previously-failing tests locally on current main before trusting the green gate. The gate validates a snapshot, not a composition.

### 2. Receipt commits shed on every re-rebase

The rebase resolution takes mains ledger + branch-unique additions. Receipt commits carrying ONLY ledger additions become empty and are dropped.

**Implication:** consolidate receipts into the feature commit before the first rebase. See #1626.

### 3. Concurrent sessions need explicit ownership boundaries

The accidental force-push on #1373 happened because two sessions worked the same branch simultaneously.

**Implication:** check lastPush timestamp before touching any branch. Force-with-lease failure is the signal to back off.

### 4. Cross-process test coordination requires OS-level locks

RwLock coordinates threads within one process. cargo test runs test binaries as separate processes.

**Implication:** the fixture coordination needs a cross-process lock (POSIX flock) or the O_TMPFILE pattern. See #1625.

### 5. GitHub mergeability is eventually consistent

mergeable_state DIRTY persisted across multiple empty commits and gate successes for #1396.

**Implication:** wait 5+ min after pushing before attempting merge. If merge fails with 0 real conflicts, push an empty commit and wait again.

### 6. --delete-branch cascades on stacked PRs

Merging a stacked PR with --delete-branch deletes the shared base branch, auto-closing all dependents.

**Implication:** check baseRefName before merging. If the base is not main, the PR is stacked.

### 7. allow.toml reason fields are append-only review logs

Conflicts between two append-only logs are resolved by: keep mains version + append branch-side Reviewed segments that main lacks.

**Implication:** union_allow.py handles this for most cases but falls back to taking mains side for non-log conflicts.

### 8. Path separator mismatches cause Windows test failures

Tests asserting error messages with file paths use forward slashes but Windows produces backslashes.

**Implication:** normalize paths before comparison. Partially fixed by #1630.

## Tooling inventory

| Tool | Purpose | Location |
|---|---|---|
| rebase_one.sh | Automated rebase with conflict resolution + gate dispatch | repo root (untracked) |
| append_receipts.py | Deduped ripr receipt appending from gate artifacts | F:/Temp/ |
| train_one.sh | Full per-PR cycle | F:/Temp/ |
| queue_drafts.sh | Sequential queue processing | F:/Temp/ |
| union_allow.py | allow.toml receipt-log union resolver | F:/Temp/ |

## PR disposition summary

| Disposition | Count |
|---|---|
| Merged | ~76 |
| Closed as superseded | 6 |
| Closed as rejected | 15+ |
| Perma-held (open) | 1 (#1384) |
| Issues filed | 5 (#1625-#1629) |
