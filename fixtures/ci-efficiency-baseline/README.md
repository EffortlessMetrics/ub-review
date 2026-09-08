# CI-efficiency before-state corpus

This offline corpus advances [#1270](https://github.com/EffortlessMetrics/ub-review/issues/1270)
within [#1268](https://github.com/EffortlessMetrics/ub-review/issues/1268).
It records selected historical observations for later comparisons. It grants
no scheduler, cache, gate, billing, or release authority and claims no savings.

## Retained sources

| PR | Exact gate run | Source shape | ZIP bytes | Expanded archive bytes |
| --- | --- | --- | ---: | ---: |
| #1261 | [33114795957](https://github.com/EffortlessMetrics/ub-review/actions/runs/33114795957) | Changed Rust test | 811,167 | 5,577,381 |
| #1263 | [33140110441](https://github.com/EffortlessMetrics/ub-review/actions/runs/33140110441) | Recorded optional coverage failure | 33,837,713 | 337,448,532 |
| #1264 | [33957570798](https://github.com/EffortlessMetrics/ub-review/actions/runs/33957570798) | Clean documentation-only change | 850,374 | 4,663,055 |
| #1266 | [33240853970](https://github.com/EffortlessMetrics/ub-review/actions/runs/33240853970) | Required proof unavailable; large artifact | 148,138,955 | 1,537,034,279 |

The manifest binds each PR head, reviewed commit, revision semantics, run,
attempt, artifact ID, and downloaded archive digest. The #1264 source is an
explicit dispatch. The other three packets reviewed a GitHub merge result;
their candidate head and admitted reviewed commit remain separate fields.

The #1263 independent baseline run `33140109246` is also represented by its
eight GitHub-owned successful check results at the exact candidate head. Its
artifact `9673611284` is expired: downloading it on September 8 returned HTTP
410. The known artifact identity/digest remains a reference, while the receipt
is explicitly unavailable and is not treated as downloaded or verified.

The dominant #1266 RIPR detail stream is 1,509,857,324 expanded bytes. The
inventory records each member's expanded size, compressed entry size and CRC,
without retaining the stream. ZIP container bytes include headers. Expanded
archive bytes include an extra `src/main.rs` member outside the packet;
`packet_expanded_bytes` counts only `target/ub-review/`. These quantities must
not be substituted for each other.

[#961](https://github.com/EffortlessMetrics/ub-review/issues/961)'s retained
#915/#916/#921 cases are referenced through the existing authority-incident
manifest's digest, Git blob and source artifact identities. Its Rust tests
provide deterministic contradiction fixtures. This corpus does not copy those
payloads or claim that the recorded #1263 sensor failure is reproducibly caused
by current code. A multi-package source and second repository remain explicitly
unmeasured; no representative packet was verified for them in this extraction.

## Measurement contract

`manifest.json` uses `ub-review.ci_efficiency_baseline.v1`. Each measurement has
a name, unit, status, basis, value and source JSON pointer, or an explicit null
value and reason. Reported packet elapsed time, legacy phase wall time, proof
command duration sum and model-call duration sum remain distinct observations.
They are not relabelled as workflow or process-tree time.

Selected public GitHub run/job metadata retains exact job and step timestamps,
conclusions, runner identity and labels. The verifier recomputes summed job
elapsed milliseconds. This is neither critical-path time nor billable minutes.
Job rounding, prices, applied Linux-equivalent multipliers, cache hit telemetry,
full process accounting and isolated queue delay are unmeasured, never zero.
Proof commands, their timeout/duration/disposition, resource leases, sensor
receipts and available TaskLedger events/snapshots remain available for a
later bounded analysis without reopening the original workflow.

Possible duplicate work stays `unproven_equivalence`. Similar command strings
cannot establish equal environment, toolchain, targets, consumers or evidence,
and cannot be counted as avoided execution. Later #896/#900/#964-#966 and #1032
own those stronger identity and economics contracts.

## Extraction, privacy and integrity

The September 8 extraction downloaded each public artifact, checked its SHA-256
and compressed size against GitHub's artifact API, and inspected its ZIP
directory. Only allowlisted small receipt members were extracted. Run and job
JSON was projected onto the listed public timing/identity fields; the digest of
the API bytes before that projection is retained as a reference, not a
signature. Packet receipt bytes were retained exactly and have per-file SHA-256
receipts. The archive digest and entry inventory identify omitted source bytes.

About 750 KB of receipts and metadata are retained under a hard 1 MiB corpus
limit and 256 KiB file limit. Full archives, source patches, raw sensor logs,
provider/model messages and prompts, delivery bodies, credentials and private
payloads are omitted. Linked/reparse files and unlisted files are rejected.
Privacy checks supplement the explicit allowlisted extraction; they are not a
general secret scanner or proof that arbitrary future fields are safe.

The semantic corpus digest sorts cases, retained-file receipts, measurements,
shape labels, incident references and omission rows. Reordering those manifest
rows or changing JSON formatting leaves it unchanged. Retained source bytes
have separate content digests; changing a source payload is an evidence change
and must update its byte receipt. No event chronology is sorted away.

## Proof and handoff

Run these repository-native offline checks:

```console
cargo test --locked --test ci_efficiency_baseline -- --nocapture
cargo test --locked --test authority_incidents
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo run --locked -p xtask -- policy-check
git diff --check
```

The new test prints the deterministic corpus digest and job elapsed sums. Its
negative cases cover forged runs/artifacts, missing and duplicate rows, wrong
units, unknown-as-zero, privacy fields and path escapes. The existing fixture
receipt in `policy/allow.toml` owns these JSON/NDJSON surfaces; no new glob or
date renewal is introduced. Exact validation receipts belong in the PR body.

Keep this PR draft until independent review and the applicable local/hosted
checks complete. Do not close #1270 until its acceptance has been checked
against these source facts and explicit gaps. Rollback can remove the fixture
verifier while preserving this historical source inventory. A later corpus
revision must retain inconvenient before-state observations rather than
rewrite them to resemble an optimization.
