# Recorded delivery observations (#959 → #1335)

This is a pure contract proposal stacked on #1395, not a live writer or a change
in publication authority. `delivery_transaction::recorded_observations` is
currently **test-compiled only**. It adds no production command or default-binary
behavior, dead-code exception, transport, persistence, retry, credentials, clock,
finalizer or gate result. #1317/#1370/#1392/#1394 remain separate frozen owners.

The pure `export_recorded_observations(ObservationInput)` boundary re-executes
#1013/#1014 validation through the existing #1393 source exporter. It returns that
original `RecordedReviewExport` unchanged, plus a separate machine observation
view. Judgment/plan original wire bytes and opaque imported CI are unchanged.
`replay_recorded_observations` independently derives and compares both objects;
repinning an altered output does not make it valid.

## Admission and source binding

The caller supplies an already admitted `PublicPlanInput`, exact prepared payload
bytes and SHA256, recorded stream bytes and SHA256, and independently retained
watermark bytes, SHA256 and requested name. Pins must not be learned from the
objects they authorize. Structural consistency does not authenticate transport
observations or supply posting permission. Source admission/classification,
receipt provenance, retained-pin authority and head-observation provenance remain
caller obligations.

`ub-review.recorded-delivery-observations.v1` has a strict subject containing the
canonical judgment repository/spec/pass/lead-attempt, delivery repository/PR/head,
RevisionRef and original source/public-source/judgment/plan/payload digests.
Source and payload identities cannot be substituted by a same-head older pass.

Prepared payload admission supports the existing COMMENT/body/inline-comments
wire subset. Every inline comment must match exactly one canonical plan delivery
location and body digest; duplicate, extra or missing inline comments fail.
Replies, suggestions, additional payload fields and partial inline reconciliation
are unsupported here. Canonical #1014 currently admits inline and summary
surfaces. **Summary body-to-payload correspondence remains unverified**; payload
summary text is not compared to the plan's summary reference. The view always
keeps whole-review delivery `not_qualified`, even when every inline is recorded
confirmed. Empty receipts cannot confirm summary or quiet silence.

## Events and ordered-prefix watermark

Each stream event is an original JSON byte vector. Its strict envelope carries a
one-based sequence, previous chain SHA256, distinct delivery-attempt ID, and one
observation:

| Observation | Effect at the named prefix |
| --- | --- |
| prepared | Registers a new attempt; remote outcome remains unknown. |
| ambiguous | Retains local failure stage/receipt digest; remote outcome stays unknown. |
| confirmed_inline | Records a canonical complete inline reconciliation for that attempt. |
| superseded | Records a different observed head; existing and later confirmations are historical. |

A retry has a new ID and an explicit predecessor with a recorded local failure or
inline confirmation. A second pending attempt without that predecessor, duplicate
attempt ID, repeated/conflicting observation, or new preparation after a recorded
head advance is rejected. Attempt uniqueness is checked within the admitted
history; an eventual producer must allocate stable globally appropriate IDs and
retain its history. No retry operation is executed by this module.

An ambiguous receipt is the exact existing `DeliveryTransaction` wire object,
validated against source head, canonical planned inline subset, terminal local
failure state and stage. It may be failed or cleaned_up; neither proves no remote
side effects. Its reason bytes are hashed and retained in input but never copied
into the observation view. Confirmation uses existing `ObservedDelivery`,
`DeliveryReconciliation` and `DeliveryReceipt`, recomputes `reconcile_deliveries`,
and requires exact equality to the original receipt relationship. This subset
also requires positive numeric physical GitHub IDs. Reuse of an identical receipt
by a reconciled retry is allowed; one physical comment cannot acquire conflicting
review/claim/head/body metadata. Canonical legacy codecs are roundtrip-checked to
reject ignored nested unknown fields.

The genesis hash domain is `ub-review.recorded-delivery-genesis.v1\0` followed by
canonical subject JSON. Each link hashes `ub-review.recorded-delivery-event.v1\0`,
the previous 64-byte hexadecimal hash, an eight-byte big-endian event-byte length,
and the exact original event bytes. It does not hash reserialized event/receipt
objects. `ub-review.recorded-delivery-watermark.v1` binds a name, nonzero prefix
count, chain SHA256 and observed head at that prefix. The requested name and its
byte pin must match independently retained expectations.

Only the named prefix is semantically admitted. A bounded future suffix cannot
change an earlier view; it does not receive receipt validation until selected by
a later independently pinned watermark. Deletion, reordering, wrong links,
conflicting events or mismatched prefix/head/name/pin reject that view. An old
watermark intentionally selected for historical replay is valid; it cannot be
substituted for a different requested current watermark. This checks recorded
prefix consistency, **not append-only storage durability or remote authenticity**.
Historical currentness names the recorded watermark, not a live GitHub lookup.

Bounds: payload/event/receipt ≤256KiB, stream/view ≤1MiB, watermark ≤4KiB, ≤128
events and inline rows, body text ≤6000 bytes. Existing canonical source-export
bounds remain in force. No local failure is projected as remote absence; later
confirmation does not erase its earlier local failure or another ambiguous retry.

## Remaining #1335 integration contract

After independent contract review, #1335 must:

1. Admit immutable recorded source files through its existing pinned intake and
   source validators. Supply the original source identity even for historical
   projection; use separately admitted remote-head evidence in the event stream.
2. Add explicit bounded regular-file pins for prepared payload, stream and named
   watermark to the intake. Retain independent watermark name/digest expectations
   and original receipt/event bytes; do not derive them from an exported view.
3. Promote this test-compiled module to the normal **pure** consumer build and
   call the API from its nonpublishing export/replay path. Preserve the existing
   source view and CI byte object; export observations as a separate view. Re-run
   actual CLI negative/mutation controls and obtain fresh exact-head review/checks.
4. Keep source currentness, local failure, recorded remote inline facts and summary
   uncertainty separate. No fabricated zero watermark, posted flag, CI result,
   production finalizer replacement or live writer follows from this contract.

A live append-only producer, authentication/retention, head observations, ambiguous
remote reconciliation, summary correspondence, reply admission and full
#959/#960/#1335 authority acceptance remain with their existing owners. Topic
source-file support remains excluded by #1395. Ancestor gate failures are not
waived by this test-compiled proposal.
