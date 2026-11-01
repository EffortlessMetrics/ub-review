# Recorded review source files (#1335)

These local commands consume independently retained source files through the
#1013/#1014 validators and #1393 export/replay API. They run no model, Git,
provider, network, posting, scheduler or production compiler operation. #1349
remains UNADOPTED. This is structural recorded-input validation, not qualification
of the full final-lead producer or a completed delivery projection.

```text
ub-review recorded-review export \
  --evidence-root <immutable-input-directory> \
  --intake intake.json --intake-sha256 <independently-retained-sha256>

ub-review recorded-review replay \
  --evidence-root <same-immutable-input-directory> \
  --intake intake.json --intake-sha256 <independently-retained-sha256> \
  --view prior-view.json --view-sha256 <independently-retained-view-sha256>
```

Retain export stdout as `prior-view.json` outside the command, without replacing
an input. Every pin is lowercase 64-digit SHA256 of exact original file bytes.
The operator must retain the intake pin independently before consumption; copying
it from an untrusted transferred intake supplies no authority. The intake author
retains upstream evidence admission, public-content qualification and subject
authority. The command proves consistency of those recorded objects, not their
origin, general privacy or actual Git-object availability.

## Intake v1

The strict JSON object has these fields. Unknown and duplicate fields fail.

| Field | Recorded input |
| --- | --- |
| `schema` | `ub-review.recorded-file-input.v1` |
| `revision` | File pin for existing RevisionIdentity canonical text |
| `graph` | File pin for existing `ub-review.claim_graph.v1` active JSON |
| `subject` | Existing JudgmentSubject: repository, review_spec_digest, pass_id, attempt_id |
| `review_complete` | Independently qualified upstream completeness receipt |
| `source_manifest_digest` | #1013 domain-separated source digest |
| `evidence` | Array of `{reference, file: <pin>}` |
| `imported_ci` | Opaque file pin, or null/absent |
| `delivery_subject` | Existing repository, pull_number, head_sha object |
| `diff` | File pin for exact diff bytes bound by RevisionIdentity |
| `public_source_digest` | #1014 domain-separated public source digest |
| `materials` | Array of `{claim_id, body_ref, class, file: <pin>}` |
| `anchors` | Array of `{reference, claim_id, path, line, line_digest}` |
| `answer` | Exact #1013 recorded judgment file pin, or null/absent |
| `plan` | Exact #1014 recorded plan file pin, or null/absent |

A file pin is `{"path":"relative/file.json","sha256":"<64 lowercase hex>"}`.
Paths use normalized relative slash components; absolute, dot, parent, backslash,
drive/colon and empty components fail. Parent components must be directories;
symlinks and nonregular leaf inputs fail before opening. The explicitly selected
root must be a directory and cannot itself be a symlink. Portable reads use
bounded descriptors and verify their bytes. Files must stay immutable throughout
the command: elimination of concurrent filesystem substitution races is not
claimed.

The graph decoder reuses existing ClaimNode/evidence/state/conflict/gap types;
their serialization and production behavior are unchanged. It requires exact
active schema and a strong revision, and rejects any fields dropped by a legacy
decoder. This first file subset **rejects nonempty topics** rather than discarding
the active topic compiler's data. It accepts existing enum wire spelling, not the
different display `key()` spellings. Topic-bearing graph admission remains with
the canonical topic owner; no Markdown conversion or alternate adjudicator exists.

File sizes: intake/revision/answer/plan <=256KiB each; graph/diff/imported CI
<=1MiB each; all evidence bytes combined <=1MiB; each material body <=6000 bytes.
Evidence/material/anchor arrays and graph claims/conflicts/gaps each <=128 rows.
Replay views <=4MiB. Bounds apply before output and during reads. No runtime config
or credentials are read; a production `.ub-review.toml` is irrelevant.

## Responses and failures

Export exit 0 prints exactly one `ub-review.recorded-source-export.v1` JSON view
and newline on stdout. It preserves original judgment/plan bytes and pins. The
opaque CI bytes remain in the independently pinned source file; the view contains
only their digest, never a parsed CI result. The command does not modify inputs.
Missing answer declared null produces `absent`; malformed/pinned-invalid answer
without a plan produces `invalid`, with no raw rejected lead copied. A declared
file that is missing is an admission error. A plan with absent/invalid lead fails.
Validated lead without a plan yields `plan_absent`. Exit 0 never means clean, CI
PASS, permission to merge, or observed public delivery.

Replay exit 0 prints a `ub-review.recorded-file-replay.v1` receipt with
`status=replayed_recorded_sources`, the exact view SHA256, shadow qualification
and `remote_effects=unknown`. It reexecutes source validation and compares the
complete prior view, even if an attacker repins altered view bytes. Original CI
is read again from the independently pinned intake; changed bytes or pins cannot
silently update the immutable view. No delivery attempts are admitted or retried.

Admission/validation/I/O failures exit nonzero and emit no validated view or replay
receipt. Diagnostics identify bounded failure categories without echoing arbitrary
malformed input values. Clap argument failures also exit nonzero. Output-write
failure may leave partial stdout; consumers must require a successful exit and
complete JSON before retaining a response. No output path is opened or overwritten
by the command.

Delivery stays `not_admitted`, watermark null, remote effects unknown, even for a
prepared or quiet plan. Currentness means consistency with the independently
admitted recorded revision, not a live remote-head query. The remaining delivery
dependency is the #765/#959/#1317/#1370 owners' versioned append-only attempt and
remote-observation contract joined to immutable judgment/plan identities at a named
watermark. Pending, ambiguous, partial, failed, confirmed and superseded projection
cannot be supplied by relabeling a legacy transport transaction.

Executable controls are in `tests/recorded_review_cli.rs`; canonical API controls
remain in the source modules. New documentation uses the existing
`policy/allow.toml#non-rust-docs-and-templates` receipt. No lint or CI-policy exception
is added; the obsolete shadow export dead-code receipt is removed.
