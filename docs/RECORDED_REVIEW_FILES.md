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
decoder. Native `ReviewTopic` values are strictly decoded and retained as source
context. Topic heads/unique claim identities and evidence class/reference/detail
must bind to the corresponding claim; topics and nested reference lists are
bounded at 128 rows. Existing enum spelling is preserved; no Markdown conversion,
alternate adjudicator or proof-link normalization occurs in admission. Recorded
`delivery`/`planned_action` strings confer no publisher or execution authority.

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

## Pending recorded AgentReviewResult composition (#1335)

The separate `recorded-review agent-export` and `agent-replay` commands compose
#1396's direct prepared-payload slot binding with #1397's recorded observation
prefix. This integration is a shadow draft while both dependency reviews are
pending. Their exact source commits are retained as ancestors; normal pure module
admission does not activate the publication writer or qualify either contract.

Use the same command flags as source export/replay. The intake schema is
`ub-review.recorded-agent-file-input.v1`, with an additional strict `observations`
object: `payload`, `stream` and `watermark` each carry root-relative `path` and
independently retained `sha256`; `payload_source_digest` pins the #1396 canonical
payload metadata; `watermark_name` is the independently requested prefix name.
The metadata binds the intake's subject, delivery subject, canonical revision,
answer/plan byte pins and payload byte pin. Do not learn these expected pins from
an exported result. Source-only commands reject this intake and its observations.

The output schema `ub-review.recorded-agent-review-result.v1` preserves exact
component objects as JSON byte arrays `source_view_json` and
`observation_view_json`; decode each array as UTF-8 JSON. The source component
retains original judgment and public-plan bytes and the imported CI digest. CI
bytes remain in their original pinned file. The separate
`prepared_payload_binding` records whole-body/comment-slot correspondence; it
cannot prove compiler origin, remote authenticity or summary delivery. The
observation component retains each attempt's local failure and recorded inline
confirmation, and distinguishes historical head status at its named watermark.
The immutable source component continues to describe its original admitted
revision; historical currentness comes from the observation component.

Qualification remains `shadow_recorded_only_pending_dependency_review`.
`accepted_intent` is `not_admitted`, `remote_authenticity` is `unverified`, and
`whole_review_delivery` is `not_qualified`. Even with an exact summary payload
binding, the observation owner's summary correspondence remains `unverified`:
there is no identity-bound physical summary receipt. Multiple summary fragments,
quiet payloads, replies and suggestions remain outside this subset.
No clean verdict, CI result, remote absence, posting permission or architecture
adoption is inferred. A later confirming observation does not erase local failure.

Admission uses existing file checks and bounds: payload 256KiB, stream 1MiB,
watermark 4KiB; the byte-array wrapper/view is capped at 16MiB. Replay independently
re-executes both validators and observation replay and compares the entire exact
wrapper, including qualifications and the binding. A repinned modified wrapper
is rejected. No live append, retry, model call, config/provider loading, network
or persistent writer is added. Source and owner reviews and hosted gates remain
separate prerequisites to acceptance.

For a retained agent view paired with another valid prefix of the same source,
`agent-replay` rejects with a content-free watermark association hint. Compare
`name`, `count`, `chain_sha256` and `observed_head_sha` in the selected intake's
watermark and retained view's `observation_view_json` watermark, then select the
matching independently pinned pair. Do not rewrite pins from view metadata.
The hint prints no labels, claim text, CI contents or identity values, validates
the selected inputs first, and does not qualify the retained view. Other invalid
inputs/views keep the generic rejection. Successful output and source bytes are
unchanged; no automatic prefix selection or authenticated recovery is added.
