# Release cut runbook

This runbook covers cutting a `ub-review` GitHub Release. The release
machinery (`release-binary.yml`) is tag-triggered and verified-ready; this
document makes the cut procedure explicit and repeatable.

The first production release is `v0.1.0` (issue #343, SPEC-0010). The `v0`
and `v0.1` tags already exist for early commit-SHA pinning but are not
release archives.

## Pre-tag checklist (prove the same commit on)

Run all five before pushing the tag. Each is a gate the release rests on.

1. **The locked CI gate is green on the target commit.** On the PR or the
   `main` HEAD you're about to tag, `ub-review/gate` must be SUCCESS.
   ```bash
   gh pr checks <PR>   # or, for a main HEAD:
   gh run list --workflow=ub-review-gate.yml --branch main --limit 1
   ```

2. **Local full gate passes on a clean checkout.**
   ```bash
   cargo fmt --all -- --check
   cargo check --workspace --all-targets --locked
   cargo clippy --workspace --all-targets --locked -- -D warnings
   cargo test --workspace --all-targets --locked
   cargo doc --workspace --no-deps --locked
   cargo xtask policy-check
   ```

3. **The packet-contract verifier self-test passes.**
   ```bash
   python scripts/verify-bun-review-artifacts.py --self-test
   ```

4. **The action smoke workflow passes on the target commit** (manual
   `workflow_dispatch` of `.github/workflows/action-smoke.yml`). This
   exercises the composite action end-to-end (`uses: ./`) without a live
   model key.

5. **(Strongly recommended) A live MiniMax model smoke run** with repository
   secrets (`action-smoke.yml` with `run_model_smoke: true`), confirming the
   BYOK provider path works against the real API.

## Cut the release

Once the checklist passes, the tag push triggers `release-binary.yml`
automatically. There is no manual build step.

The packaging job emits `release-candidate.json` with schema
`ub-review.release_candidate.v1`. It binds the archive and checksum to the
exact checked-out commit SHA, ref, tag, toolchain, asset names, and archive
digest. The tag-only publish job validates that receipt against `GITHUB_SHA`
before creating or uploading a release. Treat that manifest as the immutable
candidate boundary: documentation-only changes after the candidate run do not
invalidate it; changes to the shipped binary, action, packaging, or release
contract require a new candidate run.

```bash
# 1. Confirm you're on the verified commit.
git checkout main
git pull --ff-only
git rev-parse HEAD          # record this; it's what the release archives

# 2. Create and push the annotated tag.
git tag -a v0.1.0 -m "ub-review v0.1.0 — first release archive (Linux x64)"
git push origin v0.1.0
```

## What the workflow does (autonomously, ~20 min)

`release-binary.yml` runs two jobs on the tag push:

1. **`package`** — builds `cargo build --locked --release --bin ub-review`
   on `ubuntu-latest` with Rust 1.95.0, packages the binary as
   `ub-review-x86_64-unknown-linux-gnu.tar.gz`, emits a `.sha256` sibling,
   uploads both as a workflow artifact.

2. **`publish`** (tag-push only) — validates the tag matches
   `^v[0-9][A-Za-z0-9._-]*$`, downloads the packaged artifact, and runs
   `gh release create v0.1.0 <archive> <archive>.sha256 --verify-tag
   --title v0.1.0 --notes "ub-review v0.1.0"` (or `gh release upload
   --clobber` if the release already exists).

Monitor:
```bash
gh run list --workflow=release-binary.yml --limit 1
gh run watch <run-id>
```

## Post-cut verification

Once the workflow completes, verify the release archive before announcing it.

```bash
# 1. The GitHub Release exists with both assets.
gh release view v0.1.0
# Expect: ub-review-x86_64-unknown-linux-gnu.tar.gz + .sha256

# 2. Download and verify the checksum.
gh release download v0.1.0 --dir /tmp/v0.1.0 --pattern '*.tar.gz*' --clobber
cd /tmp/v0.1.0
sha256sum -c ub-review-x86_64-unknown-linux-gnu.tar.gz.sha256

# 3. Extract and run the binary.
tar -xzf ub-review-x86_64-unknown-linux-gnu.tar.gz
./ub-review --version
./ub-review --help | head

# 4. Confirm install-mode=release resolves the archive from a consumer.
#    (In a scratch consumer repo, or via the action-smoke workflow with
#    install-mode=release and release-version=v0.1.0.)
```

If any step fails, **do not announce the release**. Delete the tag and
GitHub Release, fix, and re-cut:
```bash
gh release delete v0.1.0 --yes --cleanup-tag   # removes tag + release
# fix, then re-run the Cut the release steps.
```

## When to advance the Bun consumer pin (separate, later)

The Bun consumer workflow (`EffortlessSteven/bun`) pins `ub-review` by full
commit SHA, currently `804d198b...`. **Do not advance it as part of the
release cut.** Per the README and `docs/calibration/bun-ub-review-ledger.md`:

> Update the SHA only after this repo's verifier passes and the Bun consumer
> workflow succeeds.

The SHA pin and a release tag are different adoption paths:
- The **SHA pin** tracks the latest known-good commit for the active Bun UB
  hunt. It moves with validation, not with releases.
- The **release tag** (`v0.1.0`) is for consumers who want the fast install
  path (`install-mode=release`).

The xtask `validate_bun_gate_pin` enforces that README / REPO_READY /
example workflow / calibration ledger all reference the *same* SHA; advancing
it is a separate PR that links the validating Bun fork PR.

## Rollback

A miscut release is recoverable but visible:

```bash
gh release delete v0.1.0 --yes --cleanup-tag   # deletes release + tag
git push origin :refs/tags/v0.1.0              # if --cleanup-tag didn't
```

GitHub may cache or index the release briefly even after deletion. Prefer
getting the pre-tag checklist right over relying on rollback.

## References

- Issue #343 — the live "cut v0.1.0" tracker.
- SPEC-0010 — the release/install contract (asset name, checksum format,
  `install-mode=release` semantics).
- `.github/workflows/release-binary.yml` — the tag-triggered workflow.
- `action.yml` — the consumer-side install path (`download_release_binary`
  with sha256 verification).
- `RELEASE_NOTES.md` — the pre-tag proof checklist (mirrored in §Pre-tag
  checklist above).
