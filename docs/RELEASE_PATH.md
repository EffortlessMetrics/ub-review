# Release path to v0.1.0

Operational document: the exact sequence from the current banked state to the
first release (#343). Every claim carries its receipt (issue, PR, run id, or
commit). Any maintainer with repo admin access can execute this end to end.

## 1. Current state (2026-06-07)

Seventeen draft PRs are banked as a stacked train, each layer independently
green and review-ready, built while merges were blocked by the run-canceller
incident (#376). Chain (child's `baseRefName` is the parent's head branch):

```text
main ← #383 ← #370 ← #371 ← #372 ← #373 ← #374 ← #375 ← #377 ← #378
     ← #379 ← #380 ← #381 ← #384 ← #385 ← #386 ← #387 ← #388
```

- #383 supersedes closed #369 — same commits (tip d0c9f53, plus empty probe
  commit 0a2a91d), republished on a fresh branch/PR as a canceller
  discriminator (kill 17 falsified branch/PR-number keying; see #376).
  #370's recorded base branch is still `extract-tools-sensors-v2` (the #369
  head); its content is identical to `tools-sensors-module` (#383).
- Test count at the stack tip (#388 receipts): `cargo test --workspace
  --all-targets --locked` 379 passed, 1 ignored; fmt/clippy clean; verifier
  `--self-test` passing; staged ripr badge-json 0 at every layer.
- Modularization stands at 8.5 of 9 modules: on `main` today `src/` holds
  `cli.rs`, `config.rs`, `builtin.rs`, `artifacts.rs`, `gate.rs` beside the
  1.4 MB `main.rs`; the stack banks `tools.rs` + `sensors/` (#383), `proof/`
  (#370), `providers.rs` + `prompt_cache.rs` (#372), `lanes.rs` +
  `observations.rs` (#374), `audit_ci.rs` + `setup_ci.rs` (#375), and
  `github.rs` (#385, step 9a). Step 9b (`compiler.rs`) is NOT banked — see
  §4.3 for why it moves jointly with the artifact verifier.

### Banked PR inventory

| PR | closes / refs | promise (title) |
|---|---|---|
| #383 | replaces #369; train step 6 | Extract tools.rs + sensors/ from main.rs (pure motion) |
| #370 | train step 8 | Extract proof/: broker, planner, and proof surface move out of main.rs |
| #371 | #312 items 1/3/4 | Proof-broker edges: absent leases fail closed; allowlist layering pinned |
| #372 | train step 10 | Extract providers.rs and prompt_cache.rs: routing and cache mode move out |
| #373 | closes the #310 remainder | Enforce `[providers.<id>].max_concurrency` with rate-limit shedding |
| #374 | train step 12 | Extract lanes.rs and observations.rs: routing and ingestion move out |
| #375 | train step 17 | Extract audit_ci.rs and setup_ci.rs: the adoption commands move out |
| #377 | #178 value-ranking half | Rank inline findings best-first after dedupe |
| #378 | closes #306 | Remove `[gate].synchronize_mode` with a dedicated deprecation receipt |
| #379 | closes #318 | Sniff cargo-allow ledger dialect at plan time; skip foreign files with a linked reason |
| #380 | closes #319 | Preflight the tokmd version pin so a drifted runner fails with the fix readable |
| #381 | #312 item 2 (last item) | Carry the patch error in base_patch_failed receipts so requesting lanes learn why |
| #384 | refs #336 (not closes) | Emit ub-review-cost.json: receipted per-run gate cost with receipted gaps |
| #385 | train step 9a | Extract github.rs: thread ingest, review posting, issue broker (pure motion) |
| #386 | closes #359 | Ingest unsafe-review-gate.json: schema-routed structured evidence, no Markdown scraping |
| #387 | closes #360 | Route comment-plan.json candidates through the compiler's single posting surface |
| #388 | closes #361 | Add recommended unsafe-review-swarm adoption config under executed-behavior test |

## 2. The block, and the one action that ends it

An external actor holding `actions:write` cancels this repo's gate runs
(`The runner has received a shutdown signal` → `The operation was
canceled.`), typically ~10 minutes into the run. Nineteen kills,
2026-06-07 09:20Z–19:33Z; full forensics on #376 (body = kills 1–12,
comments = kills 13–18). Kill 19 — run 27102328206 on PR #388 (the stack
tip), started 19:22:03Z, cancelled ~10m15s in — postdates the #376 comment
thread and shows the targeting is not limited to #369/#383 content.

Falsified by receipts (each listed with its falsifier on #376):

- billing/usage cap — #368's run passed mid-window
- PR-scoped state — fresh PR with identical content also killed
- rerun pathology — fresh `synchronize` pushes also killed
- GitHub incident — status page all-operational through the window
- OOM — temporary memory probe: 16 GB runner, ~1 GB used at kill time
- in-repo automation — no workflow holds `actions:write`; the gate token is
  contents:read / PR:write / checks:write only
- run content/duration — coverage-skipped and model-off lean runs killed;
  survivors ran 10m53s–14m39s, longer than every kill
- run-id target list — kill 16: fresh push, fresh run id, killed
- branch-name / PR-number keying — kill 17: identical commits on fresh
  branch + fresh PR (#383), killed
- workflow `timeout-minutes` — set to 60, kills land at ~10m
- overlapping-run cancellation — #375 died with no overlap; #371/#373
  survived overlaps
- transient — kill 18 came 3.5 h after kill 17; kill 19 another ~1 h later

**Unblock action (repo admin only):** Settings → Logs → Audit log, filter
`action:workflow_run` (or search "cancel"), window 2026-06-07 09:20Z–19:33Z.
Every API cancellation event names its actor (user, PAT, or GitHub App).
Identify it, stop/revoke it. No further probe runs — 19 kills is the
dataset (#376).

## 3. Merge cascade

Why this is a cascade and not a batch: the gate workflow does not pass
`base:` to the action, and `action.yml:34` defaults `base` to
`origin/main` — every PR is diffed against main regardless of its declared
base (#382). A stacked PR's pre-merge run therefore measures the cumulative
stack, not its layer: run 27093160102 (PR #373) analyzed a 16,609-line
diff, hit the ripr tool-gate at new_unsuppressed=88 (stack-cumulative), and
run 27094075649 timed out its ripr detail pass at 240s. **Only main-based
diffs are meaningful** (#382, #376 comment 14:04Z). Until #382 is fixed
(§4.2), each layer must be retargeted to main before its run is trusted.

Sequence, after the canceller is stopped (§2):

1. **Merge #383 first.** Its base is already `main`, so its diff is honest
   today. Trigger a fresh run (push; see step 2d), triage, squash-merge.
2. **Per child layer, in chain order** (#370 → … → #388):
   a. Record the old parent tip: `OLD=$(git rev-parse origin/<old-base-branch>)`
      (for #370 the old base branch is `extract-tools-sensors-v2`).
   b. `git rebase --onto main "$OLD" <branch>` and force-push. Pure code
      motion layers should rebase cleanly; a conflict means the parent's
      squash-merge differed from its branch tip — resolve toward main.
   c. Retarget the PR's base to `main` (`gh pr edit <n> --base main`).
   d. Trigger a FRESH run via the push itself. Never rerun an old run id:
      reruns of a prior run were killed even after fresh runs started
      surviving (#376 kills 14–15), and only attempt-1 push-triggered runs
      formed the survivor class.
   e. Triage the run's `sensors/ripr/exposure-gaps.json` per-finding ids
      against the layer's diff (#376 unblock sequence). With base=main the
      counts are layer-scoped, not stack-cumulative.
   f. Squash-merge (repo convention: one commit per PR on main).
3. Re-triage `docs/issue-ledger.md` as bucket items merge (ledger header
   rule); the closes-links in §1's table fire automatically on merge.

## 4. Post-cascade order

1. **Revert the three TEMPORARY workflow commits** riding in the stack
   (marked `TEMPORARY` in `.github/workflows/ub-review-gate.yml` on the
   stack branches; they land on main with the cascade):
   - b20654c — coverage-lease skip (`allow-heavy: false` shape)
   - bcc8f27 — memory telemetry probe (OOM forensics, no longer needed)
   - 6b3a9f1 — `model-mode: 'off'` (run under the kill fuse)
   Receipts: commit messages; #383 PR body lists all three as
   "to be reverted post-incident".
2. **Fix #382**: pass `base: origin/${{ github.event.pull_request.base.ref }}`
   (with a non-PR-event fallback) in the gate workflow. Its own PR with an
   executed-behavior test — it widens gate semantics (a PR whose base later
   moves gets a different diff than a main-based read), per #382 notes.
3. **Module 9b: `compiler.rs`** — the last extraction. It moves jointly with
   `scripts/verify-bun-review-artifacts.py`, whose noise-rule phrase-parity
   self-test reads `src/main.rs` by path and regex-scans it for the
   `is_*noise*` rule functions (`self_test_noise_rule_phrase_parity_with_rust`,
   ~line 5983), and whose byte-cap constants are pinned to constants "in
   src/main.rs" (~line 4366). Moving the compiler without updating the
   verifier silently skips the parity check (the file-missing branch
   returns early) — the exact mirror-drift failure mode the test exists for
   (runs 27077850477, 27073001145 cited in its docstring).
4. **Release #343 / v0.1.0**: zero releases exist today (issue-ledger §4).
   Cut the Linux x64 archive + checksums per UB-REVIEW-SPEC-0010
   (`docs/specs/UB-REVIEW-SPEC-0010-release-install.md`). Advance the Bun
   consumer's full-SHA pin only after
   `scripts/verify-bun-review-artifacts.py` passes on a downloaded run and
   the Bun consumer workflow succeeds (README "Copy/paste Bun setup";
   CLAUDE.md pin rule).
5. **Telemetry, strict order** (issue-ledger §3): #337 (suggested-fill
   ledger) → #338 (floor-time trend) → #339 (quality telemetry). #336's
   cost receipt is already banked as #384. Artifacts only by default; no
   invented quality score.
6. **Reconcile the remainder with live gate feedback**: #77 (cross-pass
   convergence + materiality threshold — the terminal sufficient/LGTM state
   already shipped) and #147 (cross-lane conflict surfacing; the
   lane-gating half folds into #76). Both are `narrowed` in the ledger;
   work them against what the post-cascade gate actually posts, not the
   original umbrella text.

Remaining routed work beyond this list lives in
`docs/specs/IMPLEMENTATION_PLAN.md` (slices 9–12 and open decisions
D3–D9); nothing there blocks v0.1.0.
