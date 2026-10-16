# RIPR module analysis-limit evidence

The deterministic sensor's existing `sensors/ripr/exposure-gaps.json` contains
an additive `analysis_limit_evidence` object, schema
`ub-review.ripr_module_limit_evidence.v1`. It is diagnostic evidence under #813,
not a finding, analyzer selection, completeness verdict or mandatory gate.
The badge remains the sole strict-zero counting authority. Existing sensor
status and deterministic gate results remain unchanged. This slice does not
implement #1014 ReviewJudgment/public authority or advance #945's serial front.

## Producer boundary

RIPR has typed AnalysisOutcome limitations, but the module-composition note is
emitted independently by `module_composition_disclosure` and the Rust language
adapter, rather than added to those typed limitations. The inspected immutable
producer is [a734bc1acc0f6ef53a3101c55a5aa9ac6fc2418e](https://github.com/EffortlessMetrics/ripr-swarm/blob/a734bc1acc0f6ef53a3101c55a5aa9ac6fc2418e/crates/ripr/src/analysis/rust_index.rs).
This contract qualifies the observed disclosure grammar, not producer binary
provenance or a released version. It does not admit a new badge/detail schema.
Prefer a separately qualified typed producer contract if this note later gains
one; do not infer adoption from a matching version string.

Parser contract `ripr.module-composition-stderr.v1` recognizes only:

```text
ripr: N Rust module composition limitation(s): CONTEXT:REASON[, CONTEXT:REASON...]; affected module contexts remain fail-closed.
```

It also accepts the exact producer's closing orientation sentence and optional
ambiguous-parent next-step sentence, embedded as literal constants in the
parser. Other suffixes are not guessed. N must be a canonical positive decimal
matching the unique listed entries, at most128. Reasons are precisely
`rust_module_ambiguous_parent`, `rust_module_cycle_or_depth_limit`,
`rust_module_context_conflict` and `rust_module_unresolved_target`; the last
requires a positive decimal declaration line in the context. Context is a
printable, nonempty display string of at most1024 UTF-8 bytes, without absolute
or parent-directory slash components. It preserves Unicode and producer escapes;
it is not decoded into a filesystem path, evidence anchor or command argument.
Ambiguous comma-separated displays/count mismatches are rejected.

## Evidence and limits

Entries retain tool, context display, reason and raw-artifact pointers. Equal
context/reason disclosures across primary badge and nested detail passes merge
into one entry retaining both source pointers. Repeated identical notes do not
mint findings. Both complete source files remain untouched, including benign,
unknown and malformed stderr. Source records expose accepted/rejected note
counts, uninterpreted-line count, UTF-8 validity, bounded prefix digest and
truncation state. A digest binds exactly the recorded prefix byte count; when
`truncated=false`, that prefix is the complete source file. It never attests
uncaptured bytes. Missing/nonregular/unreadable sources are `unavailable`.

Reads are capped at65536 bytes plus one byte to detect truncation, per source;
lines above8192 bytes and aggregate entries above128 are refused. Truncated
sources yield no interpreted notes from that source. Unsupported notes are
`invalid_disclosure`, other diagnostics `uninterpreted`, empty diagnostics
`not_disclosed`. These states do not mean analysis complete. The object always
says `completeness=not_established`, and separately retains detail status,
including `detail_unavailable`. It is generated for completed primary sensor
passes that reach detail projection; missing/crashed primary commands retain
the existing missing/failed sensor behavior without this object.

This artifact is additive preparation. It does not route limitation records
into lane prompts, readiness or final judgments, prevent current clean analysis
claims, change public review or enforce a gate. Those consumer obligations
remain with existing #1014/#813 owners under #945 ordering. Native analyzer
execution, frozen #1401 parity, installed-byte qualification, model-on review
and hosted acceptance require their own exact-input receipts. Stubbed CLI tests
prove only this adapter contract and deterministic result invariance.
