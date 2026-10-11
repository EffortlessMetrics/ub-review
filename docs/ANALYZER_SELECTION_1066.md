# #1066 preparatory blocked selection

**Not selected; Required readiness is false.** This is an inventory attachment
using #1066's existing disposition vocabulary, not a new runtime authority schema.
#1066 remains open. No tool is newly qualified, and no mirror migration is authorized.

Frozen main is `5a72634d4eec98d5de494598902cc6cac256b508`, tree
`284b5c58cd95de6ec8b6451accb4c7cf3d1abed9`. The machine packet is
[1066-blocked.json](../fixtures/analyzer-selection/1066-blocked.json).
Its generation is SHA-256 of compact JSON with lexically sorted object keys and
inventory arrays sorted by tool ID or relative path. All 13 enabled registry
rows (required and advisory), 34 inspected mirrors and the 31 existing fixture
files have explicit identities. Disabled optional sensors are excluded. Unknown
identity fields are null; they do not mean zero or accepted.

## Public identity observed on 2026-10-11

The crates.io sparse index still stops at RIPR 0.10.0. Downloading the public
0.10.0 crate independently produced SHA-256
`d49900ea4daeffc2cef5649b6c7cd4a5a82eaad306fc6f598c79628662f0b786`.
Its `.cargo_vcs_info.json` source SHA agrees with remote `v0.10.0`:
`c08d474a92d0edf97ad1e3d444ecb73fc5d439de`.
The remote tag query returned no `v0.11.0`. These observations establish package
and tag identity, **not installed executable acceptance**. No clean install or
capability execution was performed. The GitHub Releases HTTP API returned 403;
that channel remains unverified, rather than being inferred absent.

Reproduce read-only probes with the sparse index
`https://index.crates.io/ri/pr/ripr`, public archive
`https://static.crates.io/crates/ripr/ripr-0.10.0.crate`, SHA-256, archive VCS
metadata, and `git ls-remote` for both exact release tags. Index digest is an
observation identity, not an immutable future latest-version constraint.

Source hint d646155 and #7355 are partial/test-source provenance only. #813 has
no accepted applicable compatibility verdict supplied to this packet. Provider12,
config20unknown, exact-helper positive, platform/identity/suppression acceptance
and tested compatible rollback remain incomplete. Upstream #1469/#1468/#1477/
#1479 retain ship, consumer and publication ownership. #1481 is downstream and
is not a prerequisite. #1340 archive rehearsal and #1357 metadata are independent.

## Existing seams and precise gap

`src/tools.rs` emits `ub-review.resolved_tools.v1`, `ub-review.tool_status.v1`,
and `ub-review.tool_gate_outcomes.v1`. Those schemas resolve runtime tool plans,
execution availability and threshold outcomes. They do not verify a released
bundle's public archive/source digest, exact compatibility corpus or tested
rollback identity. `scripts/verify-bun-review-artifacts.py` verifies run-artifact
contracts, not release selection. No authoritative released-bundle admission
seam was found at this frozen base.

Main's badge reader still uses native/counts fallback. Draft #1347 owns its
0.5/0.6 repair. It is not a schema0.8 acceptance verdict. #1067 separately owns
badge0.8/typed0.1/raw0.2 completeness/currentness migration. Neither surface is
changed here.

Minimal integration proposal: extend the existing resolved-tool/receipt flow
through #1066's reviewed selection verifier to bind exact public package,
source, probe, compatibility/corpus and rollback receipts before any selected
disposition. Consume that one selected generation in #1067; do not add a parallel
runtime authority or infer eligibility from this attachment. Missing or conflicting
receipts must stop selection. Actual runtime negative cases belong in that seam.

## Verification boundary and #1067 handoff

`cargo test --locked --test analyzer_selection_blocked` audits packet digest,
frozen files, enabled-row completeness and rejects resealed source-only selection,
wrong-digest/version-match, missing compatibility, falsely qualified rollback,
unknown-as-zero, omitted rows, threshold changes and readiness claims. These are
**packet contradiction checks**, not proof that a runtime selector rejects those
binaries. There is no selected positive case or test-only acceptance claim.

#1067 must wait for a terminal selected #1066 packet. On that future base, re-inventory
the hashed paths: both installers and overrides; Action/workflow bootstrap;
standard-image doctor/version/install hints; xtask hints; builtin registry/config,
profiles and runtime boxes; command construction/output capture; badge/native/raw
parsers; artifact schemas/verifier; active runner/install docs and examples.
The packet deliberately authorizes zero mirror changes. Floating ast-grep differs
between npm runner and Cargo image sources; coverage install is also floating.
Other tools lack independently verified immutable artifact/capability/install
receipts. All remain explicit blockers rather than fresh qualification claims.

Preserve strict-zero config and suppression hashes. Revalidate clean public
install, platform matrix, executable shape/digest/capabilities, suppression and
finding identity, corpus compatibility and compatible public rollback before
selection. #1068 then runs the installed triggering/clean/invalid-tool controls.

Existing controlled-surface ownership: `policy/allow.toml` receipts
`non-rust-docs-and-templates` (ub-review/docs) and `non-rust-fixtures`
(ub-review/tests). This slice changes no policy receipt; existing expiry findings
remain reported by policy-check. No pins, runtime schemas, thresholds, suppression
ledgers, workflows or release behavior change.
