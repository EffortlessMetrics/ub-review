# Release preparation and cut runbook

This runbook separates candidate preparation, explicit release authorization,
publication, and consumer rollback. It describes the current
[release workflow](../.github/workflows/release-binary.yml); its existence
does not establish that a proposed candidate is ready. Repo maintenance or a
documentation change does not authorize tagging, publishing, overwriting
assets, or deleting a release.

## Current state and ownership

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
  release-only installation and negative asset cases. Active
  [PR #1265](https://github.com/EffortlessMetrics/ub-review/pull/1265) contains
  a bounded `v0.1.0` portability proof; neither its existence nor a green
  result proves general Linux support or the next candidate.
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

## Publication prerequisite: preserve the authorized bytes

The current workflow cannot yet establish #817's required identity between
the pre-authorization archive and the bytes it publishes. Dispatch and tag
push build separate archives, and packaging retains fresh filesystem
timestamps. Matching source SHAs therefore do not imply matching archive
digests. The tag job checks only its own new receipt before exposing assets.

Do not proceed to tag creation or publication until a reviewed production
path either promotes the exact authorized archive/checksum or proves
reproducible packaging and compares the rebuilt bytes to the authorized
digest **before** creating or uploading a GitHub Release. #816/#817 must
retain this prerequisite and its tamper/mismatch rejection proof. Checking
the digest after exposure is a verification backstop, not a substitute for
this missing publication boundary. [#1300](https://github.com/EffortlessMetrics/ub-review/issues/1300)
owns its implementation and proof. The remaining preparation steps below
can assemble evidence while this prerequisite remains open.

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
the resulting run's `headSha`; reject a moved ref. Retain the
`ub-review-linux-x64-release` artifact before its 14-day retention expires.
It contains the archive, checksum, and `release-candidate.json`:

```text
schema = ub-review.release_candidate.v1
head_sha, ref, tag, toolchain
asset, checksum_asset, archive_sha256
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

## Authorization and tag push

Close the publication-byte prerequisite and complete the reviewable packet
in #816 before requesting authorization in #817. The maintainer's
authorization must name the exact SHA, unused tag,
intended external actions, claims, rollback target, and any separately allowed
destructive recovery. Do not substitute a newer SHA after authorization.

Immediately before tagging, repeat the local/remote tag and release collision
checks and verify the clean checkout is still the authorized SHA. Only then
perform the explicitly authorized commands:

```text
git tag -a <authorized-tag> <authorized-candidate-sha> -m "ub-review <authorized-tag>"
git push origin refs/tags/<authorized-tag>
```

The tag-push workflow builds a new archive; it does not promote the earlier
dispatch artifact. Before publishing, it verifies that its receipt matches
`GITHUB_SHA`, the tag ref/name, Rust 1.95.0, asset names, and archive digest.
That is an identity check within the tag run, not a comparison to #816's
earlier archive. This current path must be repaired under the prerequisite
above before an authorized cut. Preserve both receipts and independently
compare the published assets with the authorized packet as #817 requires.
A mismatch is unmet release acceptance, not permission to replace the
recorded digest.

The current workflow creates a release using `.github/release-notes.md`, or
uploads with `--clobber` if the release already exists. Its tag validator
checks name syntax, not whether the name is unused or matches Cargo metadata.
The collision and version checks above are required operating safeguards;
never rerun an occupied historical tag as a routine release attempt.

The workflow publishes the archive and checksum. The candidate manifest is a
workflow artifact, not a GitHub Release asset. Neither the manifest nor the
checksum establishes signing, SBOM, provenance attestations, portability,
provider correctness, or stable-coordinator readiness.

## Independently verify the published result

Record the exact tag-push run and inspect it by ID:

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
