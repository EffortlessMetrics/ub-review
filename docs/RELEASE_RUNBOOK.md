# Release preparation and cut runbook

This runbook separates candidate preparation, explicit release authorization,
publication, and consumer rollback. It describes the current
[release workflow](../.github/workflows/release-binary.yml); its existence
does not establish that a proposed candidate is ready. Repo maintenance or a
documentation change does not authorize tagging, publishing, overwriting
assets, or deleting a release.

## Current state and ownership

> **Freshness amendment (2026-11-05):** the publication-byte prerequisite is no longer open. The authorized-byte validator ([#1343]) landed via [#1638] (`c6bc743b`) and the production publish wiring ([#1344]) via [#1647] (`500930bc`): candidate preparation is non-publishing, `cargo xtask validate-release` gates the single create, and `gh release upload --clobber` plus rebuild-on-publish are removed ([#1300] closed with the boundary proof). A v0.1.2 candidate packet (SHA `19383da1`, packaging run 42027214303) is assembled on [#816]. Remaining before a cut: the [#815] installation-proof producer and the [#817] maintainer authorization record. The 2026-09-08 table below is retained as the historical dated observation.

Read-only GitHub release and tag metadata checked on 2026-09-08 showed:

| Surface | Observed state | Claim boundary |
| --- | --- | --- |
| `v0`, `v0.1` | Historical tags, no GitHub release archives | Source references, not prebuilt distribution receipts. |
| [`v0.1.0`](https://github.com/EffortlessMetrics/ub-review/releases/tag/v0.1.0) | Published 2026-07-18 with Linux x64 archive and checksum | Historical distribution; later source and installer proof do not alter this artifact. |
| [`v0.1.1`](https://github.com/EffortlessMetrics/ub-review/releases/tag/v0.1.1) | Published 2026-08-22 with Linux x64 archive and checksum | Publication exists; broader product and portability acceptance remains separate. |
| `Cargo.toml` version `0.1.2` | Development source; no `v0.1.2` remote tag or GitHub release in this snapshot | Version metadata does not reserve a tag or prove a cut. |

The historical `v0.1.0`
[tag-push run](https://github.com/EffortlessMetrics/ub-review/actions/runs/29638670606)
failed; the release assets were completed manually, as recorded in the earlier
release notes. Do not treat that release as successful end-to-end automation.
The `v0.1.1`
[tag-push run](https://github.com/EffortlessMetrics/ub-review/actions/runs/32559138506)
succeeded at `4e7e9f0a7205b561d84f295e5a5628b44aba61a2`. These are historical
run and publication observations, not newly executed install or provider proof.

The metadata audit used source commit
`5e3a043f70f33505f62672405e2fae36b196081b`. The observed `v0.1.1` release
ID was `374860443`, annotated tag object
`1c0f2337c38edf59fea90212c639e3f8b730090c`, and peeled commit
`4e7e9f0a7205b561d84f295e5a5628b44aba61a2`. Archive asset `524783777`
was 3,177,807 bytes with GitHub-reported digest
`sha256:580bf632b479d435b6c12ae6abd53d5b80e2cbaead24fb95042ccd9e6a65790a`;
checksum sibling asset `524783778` was 113 bytes. These values came from
read-only release/tag APIs; this documentation audit did not download or
execute the archive.

Refresh this information before selecting a candidate. GitHub is the live
release board; this table is a dated observation.

- [#805](https://github.com/EffortlessMetrics/ub-review/issues/805) owns
  distribution integration and the completion criteria for its children.
- [#815](https://github.com/EffortlessMetrics/ub-review/issues/815) owns
  release-only installation and negative asset cases. [PR #1265](https://github.com/EffortlessMetrics/ub-review/pull/1265) was closed 2026-09-16 as retained `v0.1.0`-bound historical evidence, not a current merge candidate; a current-main producer for the `source_build_used=false` assertion is in flight on that issue.
- [#816](https://github.com/EffortlessMetrics/ub-review/issues/816) owns the
  next exact candidate packet after its product and stable-tool prerequisites.
- [#817](https://github.com/EffortlessMetrics/ub-review/issues/817) owns
  authorization, publication, and independent post-cut verification.
- [#1300](https://github.com/EffortlessMetrics/ub-review/issues/1300) owns
  the missing publication-byte boundary and blocks a new cut.

The occupied `v0.1.1` name in historical issue plans is not a new-cut target.
Keep unmet acceptance criteria open while selecting a fresh version; an
existing release does not retroactively supply their receipts.

## Select the candidate and prove the tag is unused

Work in an isolated clean checkout. Choose the full candidate commit SHA and
a proposed version only after inspecting current source, issues, open PRs,
and release metadata. The proposed tag must be `v` followed by the package
version reported by that candidate's binary, not a copied example below.

Read-only checks (replace placeholders with the selected values):

```text
git status --short --branch
git diff --stat
git diff
git rev-parse HEAD
git show <candidate-sha>:Cargo.toml
git show <candidate-sha>:Cargo.lock
git show-ref --verify --quiet refs/tags/<proposed-tag>
git ls-remote --exit-code --tags origin refs/tags/<proposed-tag>
gh api repos/EffortlessMetrics/ub-review/releases --paginate
```

Record the selected SHA, package version, intended tag, and results in #816.
Require both local and remote tag absence and no GitHub release with that
tag, including drafts. For `show-ref --verify --quiet`, status 1 with no
matching ref means absent; for `ls-remote --exit-code`, status 2 means no
matching ref.
Authentication, transport, API, and other command failures do not establish
absence. Stop on any collision, even if the existing tag targets the same
commit. Do not move or delete a historical tag to free a name.

## Publication prerequisite: publish only the authorized bytes

Observed through 2026-10 (before #1344): the workflow rebuilt a fresh
archive on every tag push, validated only its own self-written receipt, and
uploaded with `gh release upload --clobber` onto an existing release, so
published bytes were never bound to an authorized candidate. That
rebuild-and-clobber route is removed.

The current boundary (implemented with #1343's merged
`ub-review.authorized_release.v1` validator in `xtask/src/release_bytes.rs`
and its production adapter `xtask/src/release_publish.rs`):

1. Candidate preparation (`package` job, any trigger) is non-publishing. It
   uploads two frozen, immutable artifacts — the archive/checksum/notes and
   the authorized-release packet — pinned to run id, run attempt, artifact
   ids, and the archive digest. Ordinary preparation never creates a tag or
   a public Release.
2. Publication requires an explicit maintainer `workflow_dispatch` of
   `release-binary.yml` that carries the trusted authorization packet:
   candidate run id/attempt, both artifact ids, the exact packet digest
   (`cargo xtask release-packet-digest` derives it offline), the proposed
   tag, and the authorization record/verifier/expiry. The dispatching actor,
   not any candidate-controlled file or comment, is the source of that
   authorization (#817 owns issuance).
3. The publish job resolves producer facts independently from the GitHub API
   (run, artifacts, tree, sha-verified producer ref, tag absence, package
   version at the candidate commit), downloads the frozen candidate by
   immutable artifact id, and runs the #1343 validator before the first
   create/upload side effect.
4. The publisher creates the proposed tag at the admitted commit, the
   release, and both assets exactly once. It has no overwrite, move, or
   delete capability; occupied tags/releases, partial uploads, API outages
   and unauthorized replays stop the publish path and require a separately
   authorized recovery decision.

Matching source SHAs still do not imply matching archive digests; the
identity is the packet's exact archive/checksum digests bound to the frozen
artifact by admission, and freshness (`retained_until`, authorization
expiry) is rechecked at the boundary. #816/#817 must retain this boundary
and its tamper/mismatch/collision rejection proof;
[#1300](https://github.com/EffortlessMetrics/ub-review/issues/1300) owns the
publication-byte reconciliation and closes only from this production-boundary
proof. Release-facing description/version/claims prose remains owned by
[#1342](https://github.com/EffortlessMetrics/ub-review/issues/1342); this
section covers only the publication boundary.

## Build the pre-authorization packet

Every receipt must identify the exact candidate SHA, inputs, command, result,
and artifact. Re-query hosted `headSha` and event/ref metadata; the most recent
green run on a branch is insufficient. The current `ub-review/gate` workflow
runs on PR events and manual dispatch, so do not assume a main push produced
a new gate run.

Run the local deterministic checks on the clean candidate:

```text
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
cargo doc --workspace --no-deps --locked
cargo run --locked --package xtask -- policy-check
python scripts/verify-bun-review-artifacts.py --self-test
```

Retain hosted gate and Action-smoke run IDs with their exact source identity.
For each claimed supported provider and platform, attach the corresponding
smoke/install proof; a model-off run is not provider proof. A source build is
not release-only-install proof. Follow #816's remaining product, delivery,
replay, stable-tool, and model criteria rather than closing them from this
deterministic checklist alone.

A manually dispatched `release-binary.yml` run packages a candidate without
publishing. Dispatch against a ref resolving to the selected SHA, then verify
the resulting run's `headSha`; reject a moved ref. Retain both frozen
artifacts — `ub-review-linux-x64-release` and
`ub-review-linux-x64-release-packet` — before their 14-day retention
expires. The bytes artifact contains the archive, checksum, and offline
release notes; the packet artifact contains `authorized-release-packet.json`
with the versioned candidate identity:

```text
schema = ub-review.authorized_release.v1
repository, head_sha, tree_sha, workflow, producer_ref
run_id, run_attempt, artifact_id, artifact_name
proposed_tag, peeled_commit, asset, checksum_asset
archive_length, checksum_length, archive_sha256, checksum_sha256
binary_version, package_version, target, toolchain, retained_until
authorization_record (= release-candidate/<archive_sha256>), verifier
```

Verify the actual binary's `--version` against the proposed tag, the single
root-level executable archive layout, checksum, and supported platform.
Attach exact no-host-Cargo installation, failure-path, model-off, and supported
provider/Action receipts required by #815/#816. Record the install-proof
assertion described below for release-only paths. Retain the package/action
version mirror proof, asset names/digests, supported claims, explicit gaps,
and the previously verified consumer rollback target.

The workflow receipt binds its actual run SHA/ref/tag. A branch-dispatch
receipt's `tag` field contains that ref name; it is not evidence that a tag
exists. A documentation commit changes the SHA just as any other commit does.
Do not edit or relabel a receipt to make it describe a different commit or tag.
After any candidate movement, regenerate the exact candidate packet before
seeking authorization.

### Install-proof assertion and producer

`source_build_used=false` is the explicit assertion required in #815/#816's
harness-owned install-proof packet. It is not a current Action output or a
field emitted by `ub-review.release_candidate.v1`; current main has no
installer-receipt schema that emits this named field. The installation proof
owner must name the actual harness source revision, schema, field mapping,
and retained evidence used to establish the assertion.

That evidence must bind the selected Action ref and release artifact to the
observed resolver path, installed binary identity, environment/tool-absence
probes, and negative controls that detect forbidden source fallback. Merely
requesting `install-mode: release` cannot populate a successful receipt.
Missing observations remain not proven. PR #1265's proposed Rust harness
uses its own versioned environment/resolver receipts; its pending source and
historical runs must not be represented as a producer already on main.

## Authorization and publication dispatch

Close the publication-byte prerequisite and complete the reviewable packet
in #816 before requesting authorization in #817. The maintainer's
authorization must name the exact SHA, unused tag, the candidate packet
digest, the frozen run/artifact identity, intended external actions, claims,
rollback target, and any separately allowed destructive recovery. Do not
substitute a newer SHA or a newer packet after authorization.

Publication is an explicit `workflow_dispatch` of `release-binary.yml`
carrying the trusted authorization inputs; a tag push only re-packages a
candidate and never publishes. Immediately before dispatching, repeat the
local/remote tag and release collision checks for the proposed tag. The
dispatched publish path re-observes the run, artifacts, tree, producer ref,
tag absence and package version from the GitHub API, admits the frozen
candidate bytes with #1343's validator against the authorization packet, and
only then creates the tag at the authorized commit, the release, and both
assets exactly once:

```text
gh workflow run release-binary.yml --ref <trusted-adapter-revision> \
  -f candidate-run-id=<authorized-run> \
     candidate-run-attempt=<authorized-attempt> \
     candidate-artifact-id=<frozen-bytes-artifact> \
     packet-artifact-id=<frozen-packet-artifact> \
     packet-sha256=<authorized-packet-digest> \
     proposed-tag=<authorized-tag> \
     authorization-record=<authority-record> \
     authorization-verifier=<trusted-verifier> \
     authorization-expires-at=<epoch>
```

Dispatch the publish adapter from a revision that independently received its
own review (normally `main`): the publish job executes that revision's
`xtask` adapter with publishing credentials, and the candidate bytes enter
only as validated data. On any mismatch, occupied tag/release, expired or
unavailable candidate, untrusted authorization, API outage, or partially
uploaded release, the publish job fails with zero side effects and stays
disabled — recovery (including uploading a missing asset to a partially
published release) is a separately authorized decision, never a retry that
assumes permission to clobber or delete. Historical tags, releases, and
assets are never moved or overwritten.

The publish path uploads only the admitted archive and checksum. The
candidate packet remains a workflow artifact, not a GitHub Release asset.
Neither the packet nor the checksum establishes signing, SBOM, provenance
attestations, portability, provider correctness, or stable-coordinator
readiness.

## Independently verify the published result

Record the exact publish-dispatch run and inspect it by ID:

```text
gh run view <tag-run-id> --json headSha,headBranch,event,status,conclusion,url
gh release view <authorized-tag> --json tagName,isDraft,isPrerelease,publishedAt,assets,url
git ls-remote --tags origin refs/tags/<authorized-tag> refs/tags/<authorized-tag>^{}
```

Verify the annotated tag's peeled commit equals the authorized SHA. Download
the actual release assets into a new scratch directory, recompute the archive
checksum, compare names/digests to the packet, inspect layout, and execute the
extracted binary on each supported platform. Record the actual binary version,
`--help`, model-off smoke, supported provider/Action smoke, and negative asset
results. Strict `install-mode: release` must use the published asset and must
never silently compile from source.

Capture exact release/asset IDs, platform and tool versions, run IDs, and
retained receipts in #817. Complete #805 only when its children and remaining
integration acceptance are proven. Do not announce or use a failed or
unverified candidate for pilots or stable-coordinator adoption.

## Consumer rollback and exceptional release removal

For a failed candidate, preserve evidence and stop rollout. Consumer rollback
means returning the affected consumer to the previously verified immutable
release or source pin recorded in the packet, with its required validation.
The Bun pin has its own verifier and consumer-run requirements in the
[calibration ledger](calibration/bun-ub-review-ledger.md); a release does not
advance or roll it back automatically.

Deleting a GitHub Release, deleting or moving a tag, or overwriting published
assets is a separate destructive action. It requires explicit authorization
for the identified objects; ordinary release-prep work and a failed smoke do
not supply it. Preserve history and prefer a fresh version for a corrected
candidate. If removal is separately authorized, record the exact removed
objects and retained failure evidence, then regenerate #816's packet and
obtain fresh #817 authorization for the next candidate.

## Related source truth

- [SPEC-0010](specs/UB-REVIEW-SPEC-0010-release-install.md): install contract.
- [Release notes](../RELEASE_NOTES.md): published versus development state.
- [Product state](PRODUCT_STATE.md): current runtime and acceptance boundaries.
- [Branch protection](ci/branch-protection.md): required gate and independent
  containment limits.
- [#1293](https://github.com/EffortlessMetrics/ub-review/issues/1293): this
  release-state reconciliation; it does not authorize a cut.
