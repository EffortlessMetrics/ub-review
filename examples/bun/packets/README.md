# Bun Packet Examples

These records preserve real Bun fork packet behavior without vendoring full
GitHub artifact downloads. Full artifacts can expire; these examples keep the
review contract, source PR, run id, and expected compiler behavior available to
future lanes.

Use these records when changing the Bun profile, verifier, compiler, or PR body
policy. They are calibration examples, not upstream Bun correctness claims.

## Example Matrix

| Example | Source | Expected gate behavior |
|---|---|---|
| Workflow smoke success | `EffortlessSteven/bun#29`, run `26843669614`, attempt 2 | strict verifier passes; 10 MiniMax lanes plus refuter ok; grouped review posts with no off-diff comments |
| Provider preflight failure | `EffortlessSteven/bun#29`, run `26843669614`, attempt 1 | MiniMax HTTP 500 is recorded as missing model evidence; no clean-review claim |
| Artifact-only sufficient | `EffortlessSteven/bun#49`, run `26991938752` | terminal state `sufficient`; `github-review-skip.json` and `post-result.json` prove no reviewer-value PR body |
| Verification-question review | `EffortlessSteven/bun#28` human calibration | repeated lane concerns merge; false premises are dismissed; actionable route/test questions survive |

## Workflow Smoke Success

Source:

```text
repo: EffortlessSteven/bun
pr: 29
artifact: ub-review-packet-29
run: 26843669614
attempt: 2
digest: sha256:2ec165d6487ef1f9c78999e0cf25ce6be4ead78762c68a8acaf4f5b3a9c6ac24
runtime: about 2m14s
inline_comments: 3
model_lanes: 10 expected MiniMax lanes ok, refuter ok
missing_model_evidence: 0
off_diff_comments: 0
```

Expected behavior:

- verifier accepts the packet with the strict Bun model-lane expectation;
- grouped PR review post succeeds;
- inline comments stay on diff;
- successful lane/provider status remains artifact-only unless it changes the
  reviewer's decision.

## Provider Preflight Failure

Source:

```text
repo: EffortlessSteven/bun
pr: 29
run: 26843669614
attempt: 1
failure: MiniMax preflight HTTP 500
```

Expected behavior:

- provider failure is missing model evidence;
- the packet is not rendered as a clean or sufficient review;
- artifacts preserve the failed provider evidence for diagnosis;
- PR body text does not turn provider status into generic residual risk.

## Artifact-Only Sufficient

Source:

```text
repo: EffortlessSteven/bun
pr: 49
artifact: ub-review-packet-49
run: 26991938752
terminal_state: sufficient
posting: artifact-only skip
```

Expected behavior:

- `github-review-skip.json` exists when no reviewer-value content survives;
- `post-result.json` records the artifact-only skip;
- no no-finding boilerplate is posted;
- the verifier treats artifact-only sufficient as a successful gate state.

## Verification-Question Review

Source:

```text
repo: EffortlessSteven/bun
pr: 28
surface: Markdown resizable ArrayBuffer review calibration
```

Reviewer-value questions that survived human calibration:

- Does `Markdown.react` run the relevant option getter before constructing the
  `&[u8]`?
- Does a typed-array view over a resizable ArrayBuffer preserve the resizable
  flag into `PinnedView`?

Expected behavior:

- duplicate missing-witness caveats merge before the final review;
- source-only assertions do not become proof;
- false premises such as recoverable `Box::<[u8]>::from(slice)` allocation
  failure are dropped or refuted;
- route/test questions survive when they can change the upstream decision.

## Update Rule

Add a new example only when there is a concrete Bun PR, run id or review record,
and a behavior that future verifier/compiler work must preserve. If the full
artifact is available, record its name and digest. If it is not available, state
the exact evidence source and do not invent packet fields.
