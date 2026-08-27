# Authority incident fixtures

`fixtures/authority-incidents/` is the permanent, offline evidence corpus for
issue #961. It retains the smallest byte-identical artifact set needed to
reproduce the authority contradictions observed in exact-head hosted runs for
PRs #915, #916, and #921. The corpus is evidence input for #957; it does not
define the current TaskLedger or gate schema and does not make historical
behavior correct.

## Provenance

| PR | Head | Workflow run | Artifact | Archive digest |
|---|---|---:|---:|---|
| #915 | `c2ec05d3f5a5cd1c82b79f1f9824f5a2cc23642d` | `32593806467` | `9481061943` | `sha256:dbd084ca66eb789b6a6d6aaf476388d5beb2fe8615ea03c49c89968ecdef282f` |
| #916 | `e201b672d61b26563a8c7e55da0996093a83b0cb` | `32542016728` | `9467384189` | `sha256:816569eaa9182bdb8166389b3a6d2692f9de78022548464caadf654cd4bf1b55` |
| #921 | `31e741db641ccfc6057511ebc0d8b2f2521ee54f` | `32628010111` | `9490287288` | `sha256:df1f4d1d6d71821bd7af1cdc9068c05232d29e3acb6a65d13c829ad41c6f9eb5` |

The complete source archives were available and unexpired on 2026-08-27. They
were downloaded with GitHub CLI 2.86.0. The committed manifest records the
source base/head revisions, extraction identity, selected-file byte counts and
SHA-256 digests, intentional omissions, and expected invariant violations.

No retained file was edited or redacted. Instead, selection excludes model
transcripts, prompts, source patches, delivery payloads, bulk logs, and
unrelated sensor output. The loader rejects recognized secret markers and raw
private-payload keys as a second boundary.

## Deterministic extraction

From a clean checkout, download each named artifact into a separate directory:

```text
gh run download 32593806467 -n ub-review-gate -D <source>/915
gh run download 32542016728 -n ub-review-gate -D <source>/916
gh run download 32628010111 -n ub-review-gate -D <source>/921
```

Copy only the paths declared for that case in
`fixtures/authority-incidents/manifest.json`, preserving their bytes. Compare
each file's byte count and SHA-256 digest to the manifest, then run:

```text
cargo test --locked --test authority_incidents
```

Unchanged source inputs reproduce the committed file digests. The loader also
rejects missing or undeclared files, unsafe paths, symlinks, digest or size
drift, an exceeded 64 KiB corpus budget, sensitive markers, malformed JSON,
unknown manifest fields, and evidence pointers that no longer resolve.

## Retained contradictions

- #915: legacy `conclusion: pass` while all three matched Required requests had
  no passing receipt and were recorded in `not_proven_reasons`.
- #916: legacy `conclusion: pass` while the truthful `gate_result` was
  `not_proven` and all three Required requests were skipped.
- #921: impact-planner receipts were absent from queue and portfolio task IDs;
  cargo-allow completed successfully while its queue task remained `planned`;
  Required tasks had no selected task or receipt; symbolic `HEAD` remained in
  proof authority; and timeout ceilings exhausted the reported proof budget
  despite shorter actual execution and a positive deadline remainder.

The manifest enumerates cases and evidence paths so #957 can consume the corpus
without incident-specific directory discovery. Reconciliation logic must still
define and test the generic invariants; this corpus only supplies immutable
historical inputs.
