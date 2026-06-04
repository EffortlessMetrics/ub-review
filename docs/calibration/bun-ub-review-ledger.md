# Bun UB Review Calibration Ledger

This ledger tracks `ub-review` behavior on Bun fork PRs. It is for tuning the
review compiler and prompts, not for upstream Bun claims.

## PR #29 Blob shared-buffer smoke

Date: 2026-06-02
Repo: `EffortlessSteven/bun`
Artifact: `ub-review-packet-29`
Run: `26843669614`, attempt 2
Digest:
`sha256:2ec165d6487ef1f9c78999e0cf25ce6be4ead78762c68a8acaf4f5b3a9c6ac24`
Runtime: about 2m14s for attempt 2 job execution
Inline comments: 3
Model lanes: 10 expected MiniMax lanes ok, refuter ok
Missing model evidence: 0
Off-diff comments: 0

Acted on:

- The smoke proved the `ub-review` GitHub Action resolves and runs from the Bun
  fork workflow.
- The strict verifier passed with all 10 expected lanes.
- The grouped review posted successfully with HTTP 200.

Dismissed:

- Attempt 1 was not a valid product proof because MiniMax preflight returned
  HTTP 500. The artifact still behaved correctly by recording failed model
  evidence instead of reporting a clean review.

Parked follow-ups:

- Review efficiency metrics should record wall-clock time, model lane counts,
  provider failures, off-diff rejections, body size, and post status.
- Profile extraction should preserve this v0 behavior as
  `profiles/bun-ub-v0.toml`.

## Current Bun gate pin

Date: 2026-06-04
Repo: `EffortlessSteven/bun`
PR: `#46`
Pin: `EffortlessMetrics/ub-review@7b969e53b58d7b2a32db9006f1f2f43916fc2134`
Run: `26957379285`
Artifact: `ub-review-packet-46`

Acted on:

- The Bun workflow pin advanced after `ub-review` PR #209 and PR #211.
- The `UB evidence packet / gh-runner` job passed on the new pin.
- The packet artifact uploaded and was not expired at merge time.
- The packet verifier passed with zero inline comments and `tokmd` status `ok`.
- The packet includes `evidence_stream_started` for the role-stream event
  contract.

Prompt/compiler follow-up:

- Keep provider failures under missing or failed evidence only.
- Keep the strict verifier as the release proof for Bun fork runs.

## PR #28 Markdown RAB review coordination notes

Source: human review of the first useful multi-lane Bun UB output.

Useful verification questions:

- Does `Markdown.react` actually run the relevant option getter before the
  `&[u8]` is constructed?
- Does a typed-array view over a resizable ArrayBuffer preserve the resizable
  flag into `PinnedView`?

Repeated observations:

- Test/build/Miri witnesses were skipped.
- Missing SharedArrayBuffer or negative-path tests were mentioned by several
  lanes.
- Resizable-flag propagation appeared as the same verification question in
  multiple forms.

Dismissed:

- `Box::<[u8]>::from(slice)` does not fail by returning `None`.

Prompt/compiler follow-up:

- Add an observation ledger so repeated lane concerns merge before summary
  rendering.
- Add a false-premise confirmation path before allowing this class of finding
  inline.
- Treat skipped witnesses as one global missing-evidence observation, not one
  caveat per lane.

## Known calibration item: allocation-failure false premise

Source: prior Bun UB review run discussion.

Dismissed:

- Do not claim that `Box::<[u8]>::from(slice)` can fail by returning an empty
  box or a `None` fallback. Allocation failure is not a recoverable branch for
  that API shape.

Prompt/compiler follow-up:

- Teach the refuter to demote findings that invent recoverable allocation
  failure branches for infallible allocation APIs.
- Prefer summary-only or dropped disposition when a finding depends on such a
  branch.
