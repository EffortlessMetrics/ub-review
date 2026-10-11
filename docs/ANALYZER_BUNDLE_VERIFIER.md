# Recorded analyzer bundle verification (#1066)

`ub-review verify-analyzer-bundle` is a nonpublishing production entrypoint for
joining independently retained selection evidence. It reads transferred bytes,
checks exact identities, and prints a deterministic receipt on stdout. It never
installs or executes an analyzer, changes pins, modifies Required policy, or
accepts a visible version string as proof of public installation.

```text
ub-review verify-analyzer-bundle \
  --evidence-root <transferred-evidence-directory> \
  --intake intake.json \
  --intake-sha256 <independently-retained-intake-sha256>
```

Exit 0 means `selected_recorded_receipt`: all recorded selection obligations
agree with independently pinned inputs. Nonzero emits one explicit
`not_selected`, `missing_evidence`, `stale_evidence`, or
`contradictory_evidence` receipt. Rejected bundles emit no validated tool list.
Every result has `required_readiness=false`. This command is not live runtime
admission or public-candidate qualification; #1067 migration and #1068 installed
triggering acceptance remain separate. The held #1385 packet is unchanged.

## Authority and trust boundary

The operator retains the intake hash independently before consuming evidence.
The intake pins exact bundle, canonical `ub-review.resolved_tools.v1` bytes and
separate source-owned public/install/compatibility receipt envelopes. Do not
copy these pins from an untrusted bundle. The intake owns the frozen existing
`RevisionRef`, corpus/policy/suppression hashes, platform, supported output
schemas/platforms, source-owned package validator revision and exact current
released rollback identity. There is no independent version list, badge parser,
threshold evaluator, or alternative tool registry here.

The verifier projects enabled/required tool roles from the pinned resolved-tools
receipt. Every enabled row, including advisory rows, must occur exactly once in
bundle and intake. Disabled rows do not qualify a new tool. Unsupported source
or output/platform contracts fail closed. This initial bounded implementation
supports Cargo's current crates.io production source only; npm, Go and GitHub
asset installation are unsupported until their owners provide matching proof.
It does not implicitly certify the entire current UB Review bundle.

Upstream #7311 retains package archive metadata, exact installed version/source
and quiet-process qualification. The install envelope retains byte-pinned raw
upstream validation evidence and the separately approved validator source SHA.
This verifier does not reproduce that archive/version parser. #1347 retains
badge0.5/0.6 admission; #1067 retains badge0.8/typed0.1/raw0.2 completeness and
currentness migration. Existing badge or gate outcome schemas are untouched.
#813/#1468/#1477 retain compatibility and corpus acceptance. #1469/#1479 retain
public ship and publication evidence. No dependency on downstream #1481.

Recorded envelopes are a minimal selection-specific intake contract. They are
not claims that upstream currently emits these new envelope schemas. A reviewed
export/adapter must retain the original source-owned evidence and independently
pin each envelope before real consumption. In particular, a caller-controlled
`public_verified=true`, `decision=accepted` or `origin=public_registry` cannot
establish authenticity: these statements are trusted only because their exact
bytes and producer identity were admitted by the upstream owners and operator.
The verifier checks consistency with that retained authority, not signatures,
remote registries, producer execution or whether a fabricated pin is truthful.

## Recorded object layout

All references are `{path, sha256}` and resolve inside evidence-root. Absolute,
parent-traversal and symlink paths are rejected. JSON/probe/producer objects are
limited to 512 KiB; each archive/executable is limited to 32 MiB. Larger objects
are rejected explicitly, not considered qualified. Inputs should be an immutable
transferred snapshot; this portable reader does not claim elimination of
concurrent filesystem substitution races.

`ub-review.analyzer_selection_intake.v1` contains:

- `scope`: revision, corpus/policy/suppression SHA256 and platform;
- `resolved_tools`, `bundle`: separate object references;
- `public_identity_owner`, `install_probe_owner`, `compatibility_owner`;
- `tools`: unique IDs, supported schemas/platforms, package-validator source,
  current released identity, and candidate/rollback receipt reference sets.

`ub-review.analyzer_bundle.v1` contains the same scope and unique tool rows:
`id`, `required`, #1066 disposition, candidate and rollback identities. Only
`selected_required`, `retain_current_required`, `selected_advisory_only` are
potentially accepted; explicitly non-selected #1066 dispositions remain stopped.
Retain-current requires the candidate equal the independently pinned current
release. Rollback must equal that same current identity and pass its own public
installation, probes and qualified compatibility proof.

An identity binds package/version/public crate URL, source SHA/tag SHA,
archive/executable references, output-schema/capability SHA256, and exact probe
argv/stdout hashes. Source and tag must agree; version alone is insufficient:
at least a version probe and a distinct capability/command probe are required.
No probe is executed by the verifier.

The public envelope `ub-review.analyzer_public_identity.v1` binds owner/tool,
package/version/source URL/source SHA/exact tag/tag SHA, archive SHA256 and
public verification. The install envelope
`ub-review.analyzer_install_probe.v1` binds owner/tool/version, upstream package
validation object/source, frozen scope, public registry origin, clean prefix,
no source checkout, exact locked public install argv, archive/executable/schema/
capability identities, and successful exact executable-bound probe records.
Probe stdout bytes are independently hashed. The compatibility envelope
`ub-review.analyzer_compatibility.v1` binds owner/tool, frozen scope, candidate
archive/executable, current baseline archive, accepted decision and rollback
qualification. Null/missing receipt references are missing evidence, never zero.

## Proof and current limitations

`cargo test --locked --test analyzer_bundle_cli` drives the actual binary.
Synthetic archives, executable snapshots and recorded probes have independently
computed hashes; they are never installed or run. The positive proves only
validator behavior with consistent retained records, not a real public release.
Negative controls change bytes or reseal a single wrong/stale/missing field so
failure is observed at the intended production rule.

Real RIPR public identity remains 0.10.0. No 0.11.0 release/tag, accepted exact
compatibility, qualified rollback or current full platform proof is presumed.
No real public candidate is selected by this PR. Source-only evidence and
workspace substitution cannot become public qualification through this CLI.

Controlled documentation uses the existing
`policy/allow.toml#non-rust-docs-and-templates` receipt (ub-review/docs).
New code/tests are Rust. No policy receipt, pin, threshold, suppression,
workflow, credential, release or security setting changes.
