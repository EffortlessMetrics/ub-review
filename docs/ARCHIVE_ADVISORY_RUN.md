# Preliminary archive advisory-run campaign (#1341)

Owner: release acceptance tests. This file and `tests/archive_advisory_run.rs`
are the complete slice. Installation remains with #1382, byte validation with
#1379, publication with #1344, and result producers with their existing owners.

The executable Rust campaign consumes the strict #1382 frozen-archive contract:
`UB_ARCHIVE_PACKET` contains the preliminary Linux archive, checksum and
`manifest.json`; `UB_ADVISORY_INSTALL_RECEIPTS` contains a completed
`ub-review.archive_onboarding_rehearsal.v1` `bounded_pass` receipt and installed
`runtime/ub-review-action-release/ub-review`. It checks all frozen digests and
manifest equality, then invokes that installed executable. It does not install
or rebuild the executable. Source/toolchain manifest labels are not provenance.

```sh
UB_ARCHIVE_PACKET=/absolute/frozen-packet \
UB_ADVISORY_INSTALL_RECEIPTS=/absolute/strict-install-receipts \
UB_ADVISORY_RECEIPTS=/absolute/new-campaign-receipts \
cargo test --locked --test archive_advisory_run \
  frozen_archive_advisory_campaign -- --ignored --nocapture
```

The output directory must be new. The harness requires Linux x86_64,
`/usr/bin/git`, `/usr/bin/python3`, `uname` and `sh`. Its children receive a
cleared environment and an allowlisted PATH. The only curl implementation is
an inert synthetic adapter with exact `.invalid` URL routes; unknown routes
exit without networking. Provider and GitHub credentials are synthetic. The
Rust harness itself runs under Cargo; reviewed fixture commands have no Cargo
or rustc available. Each child has a 30-second direct-child kill/reap boundary,
not a general process-tree containment claim.

`receipt.json` records the selected archive/binary/manifest/install receipt,
input diff/source hashes, verifier/harness hashes, exact argv, policy/provider
configuration, stdout/stderr hashes, result artifacts, payload, transport logs
and delivery receipts. A wrong expected conclusion must reject every emitted
run result. Successful submission must retain pending review identity 987;
pending creation is HTTP 201 and submission HTTP 200. A separate mismatched
ID 988 control records legacy CLI acceptance but must be rejected by the
campaign identity oracle. This does not qualify the legacy producer: #1317
already owns production identity rejection. Git revision/diff commands must
exit successfully before their output becomes input evidence; the ordinary
regression checks all three operations against a real non-repository. The automatic test independently discriminates wrong result planes
and rewritten imported outcome bytes. Failed campaign attempts retain their
receipts and logs; intake errors reject before accepting any campaign evidence.

| Slice | Executable observation | Acceptance boundary / owner |
|---|---|---|
| Model off | No provider calls; deterministic current result planes retained | Current producer reports gate `not_proven`; disabled applicability remains #1352/#1373 |
| Required evidence absent | Intelligent CI conclusion `inconclusive`, gate `not_proven` | Missing evidence cannot become a clean sufficient gate |
| Structured provider finding | Public summary payload prepared; private audit positive control retained only privately | Current verifier rejects resolved-candidate reconciliation; producer also labels merely prepared payload `publication_result: posted` (#959); both blockers |
| Provider outage | One synthetic preflight call; analysis and gate `not_proven` | No live provider qualification |
| Unstructured provider text | Two synthetic calls; degraded lane retained and analysis limited | Text is not promoted to findings |
| Missing credential | No transport invocation; current missing-input outcome | Synthetic environment contains no actual credentials |
| Prepared versus delivered | No post-result before posting; separate CLI post yields submitted synthetic review | #959 remains delivery authority; this exercises no actual GitHub endpoint |
| Delivery failure | Failed submit, cleanup path, nonzero strict post exit; original deterministic bytes unchanged | Existing error receipt reports `failure_stage: unknown` and null HTTP status despite adapter HTTP 500; producer gap remains open |

The packet verifier is invoked for all six run packets. Five must succeed.
For the selected binary SHA-256 below, the structured-finding case must fail
with the exact current
`resolved_candidates.json does not match candidates plus follow-up outputs and
prior resolved candidates` rejection. Verifier success also fails this frozen observation; changed binary bytes
require refreshed expectations. The additive diagnostic imports the unchanged
verifier and runs its full sequence, retaining only the exact reconciliation
denial and the prepared-stage `neither post-result.json nor post-error.json
exists` denial that the first failure previously masked. It executes the
remainder of those functions and rejects every other failure. Its output says
the full packet remains rejected. A copied structured packet with
`CONTROL_INVALID_SCHEMA` must fail this diagnostic; an actual emitted-packet
mutation independently proved the full campaign now rejects that corruption. The final status is deliberately `bounded_observations_with_blockers`.
The prepared payload is checked before either synthetic post and already says
`publication_result: posted`; this is an unsupported producer label, not delivery
confirmation. A future producer fix must update this explicit observation and demonstrate
successful verification rather than silently inherit a positive claim.

This campaign does not qualify final #816 archive bytes, signatures/provenance,
platforms, real Actions, live providers/delivery, or terminal/outcome authority
#957–#962. It does not execute an actual deterministic source-defect sensor:
required-evidence failure and imported result invariance are the bounded
controls. Joined privacy unit proof #1376 remains independent evidence.
#1217 DevOps is not an advisory-release prerequisite.

## Local evidence, 2026-10-11

Selected source: `777d86a0fef6702f76502dd1c959c97d43600682`, tree
`b08c618989c3ea94936683810bcab3af5d0ee34a`. Preliminary local release-profile
build: `cargo build --release --locked`, Rust 1.95.0, x86_64 Linux.
Strict-install harness: #1382 head
`26402b044a57745ee4168b27e62329b041fa391b`; 17 scenarios/41 commands passed.
Advisory campaign: eight scenarios/25 commands; five packet verifier passes
and one explicitly retained structured-finding rejection. Runtime 4.77 seconds.

| Evidence | SHA-256 |
|---|---|
| Selected archive | `120baf78e8b810a77496a10621e2e1c6c50884756636be93ff49fa30fc491600` |
| Installed binary | `7b0379029b108a264222e01cabf58a0f3c471ae845bb24b2cebea1f75750c8b6` |
| Strict-install receipt | `e8b291b30abaa7cda3e42b33dc1b31470a082e3e60eed6f68f466b2f927655e6` |
| Campaign receipt | `86894e8534287490ddc6229f55bdcaba04d9430c8430fdac72eb703fea4c307a` |
| Campaign harness | `6cd7f4e86fec940a0748eb660cad16876d47a634498e221f884a23db60d2f566` |
| Inert transport adapter | `450806daf64a6f399c7202ea82b7eb795908248110d577e56f0ad82272b224d7` |

Raw campaign evidence is retained at `/tmp/advisory-run-receipts-reviewed` in
this executor, installation evidence at `/tmp/advisory-strict-install`, and
selected inputs at `/tmp/advisory-selected-archive`. These local files are not
hosted release assets. Earlier harness failures remain in separately numbered
receipt directories. Baseline local fmt/check/Clippy/full tests/docs/policy/
verifier self-tests passed. After the bounded review adjustments, the campaign
and Clippy passed again. The archive test is intentionally ignored by ordinary
Cargo test because the selected archive and strict-install evidence are explicit
external inputs. The automatic oracle discrimination test runs ordinarily.


## Review repair evidence, 2026-10-11

Original head `e60f89125caf9a520453f2fc05eb376b62961fff` and the receipt table
above remain historical evidence. The repaired harness preserves the selected
archive/binary/strict-install digests and the preliminary source scope. Eight
scenarios now execute 28 commands, including independent unaffected-contract
and invalid-gate checks plus the mismatched identity observation. Five ordinary
run packet verifications succeed; the structured packet's ordinary verification
still rejects its exact documented reconciliation gap. The supplemental check
retains the precise prepare-stage missing-post denial as an additional observed
boundary, never a complete packet pass.

Final repair campaign `/tmp/1391-repair-final-v2` passed in 5.42 seconds. Its
receipt SHA-256 is
`090b58a26df1f6272b76c67944bda6972e6ca8d4d20d49d049c707b863dc6ad5`;
harness `2068491d977618a82817165159a6089cbc48ec4f5d4a4d3b4add1ad905570ac2`;
transport adapter `f0d9dc76caeb02e61d64cba89c3da76681d8f8d7dc8b4b82e6f8507e538ff3a2`;
supplemental driver `766ed2ed4e113d1cf8ce262d221406fdabfaf28d5cc818105492d56032416eb6`.

Discriminating evidence: `/tmp/1391-invalid-gate-mutant.log` is RED with the
exact invalid-schema rejection after mutating the actual structured output;
`/tmp/1391-git-fail-open-mutant.log` is RED after bypassing Git exit checking.
Both disposable mutations were restored. Ordinary tests also reject unexpected
verifier success, an unrelated or additional denial, and widening the known
gap to other modes. No production, verifier or strict-install code changed.
HTTP-status extraction remains with the resumed #1370/#1317 owner task;
its actual native HTTP500 regression is separately RED on owner87e28f8.


## Identity-negative specificity follow-up

The negative observation now requires exactly `988/COMMENTED`, then checks
rejection against identity `987` alone. `987/PENDING` and `987/COMMENTED` both
match that identity and cannot count as an identity-negative control; the
automatic regression rejects both as negative evidence, and also rejects
`988/PENDING`. Restoring the old combined identity/state predicate is RED on
`987/PENDING` (`/tmp/1391-identity-specificity-old-oracle-red.log`), then restored
GREEN. No production or other campaign behavior changed.

The complete eight-scenario/28-command campaign remains
`bounded_observations_with_blockers` at
`/tmp/1391-identity-specificity-campaign`. Receipt SHA-256
`c1142c7c013903f653385caf0af93a257d813df0a6d3cd565f6893d4fcccc78e`; harness
`7969c613b54071f382fb7496bf0e9a40ce4aee999fb0baa67d0abb2c7f5b37c1`. Four automatic tests,
focused Clippy and fmt passed. Previous b7a41619 evidence is preserved above.
