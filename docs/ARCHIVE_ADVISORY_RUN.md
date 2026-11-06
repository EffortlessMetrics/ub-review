# Archive advisory/delivery qualification campaign (#1341)

Owner: release acceptance tests. This file and `tests/archive_advisory_run.rs`
are the recovered slice: the closed PR #1391 campaign branch
(`test/archive-advisory-campaign`) re-applied onto current main and adapted to
the current advisory/recorded-review surface, not a wholesale transplant.

## What qualifies what

Until this campaign lands, main IS the candidate content. The deterministic
scenarios therefore drive the source-built `ub-review` binary
(`CARGO_BIN_EXE_ub-review`) and their observations qualify candidate content
directly. The archive-bit-for-bit qualification is a separate, retained
packaging step: fresh archive bytes built from this tree differ from the
retained packaging artifacts only in embedded timestamps, and the runbook
route qualifies those retained bytes; this campaign does not rebuild or
re-verify archive containers.

The executable campaign requires Linux x86_64 (`/usr/bin/git`,
`/usr/bin/python3`, Unix permissions), so it executes under the repository
gate and any Linux host; the pure oracle tests in the same file run on every
platform. `cargo test --locked --test archive_advisory_run` is the validation
entry point on both.

## Seven-case coverage map

| # | Maintainer case | Named test / scenario | Mode |
|---|---|---|---|
| 1 | Normal model review producing a usable conclusion | deterministic: `campaign::source_candidate_archive_advisory_campaign` scenario `structured-finding` (usable `pass`/`findings` conclusion, packet contract verified); live: `oracle::smoke_receipt_consumer_rejects_incomplete_or_misattributed_smoke` controls + the ignored hosted consumer run against the Action-smoke model artifact | deterministic scripted + live hosted |
| 2 | Approved provider fallback completing review | `source_candidate_archive_advisory_campaign` scenario `provider-fallback`: primary preflight 503, review completed on the approved fallback (`opencode-go`) lane with `fallback_from` naming the primary | deterministic scripted |
| 3 | Total provider failure or unusable model output | scenarios `provider-outage` (HTTP 503, one preflight call, `not_proven` planes), `provider-text` (unstructured text retained as degraded lane, analysis `limited`), `missing-key` (no credential, zero transport calls) | deterministic scripted |
| 4 | Deliberate model-off operation | scenario `model-off` (`--model-mode off`, zero transport calls, `artifact-only` planes) | deterministic scripted |
| 5 | Required deterministic evidence failing / unavailable | scenario `required-gap` (`tokmd` required but disabled in intelligent-ci; conclusion `inconclusive`, gate `not_proven`) | deterministic scripted |
| 6 | Prepared GitHub delivery versus confirmed delivery | scenario `structured-finding` prepared-phase check (no `post-result.json` before posting), then the in-packet confirmed post (pending review 987 → submitted COMMENTED, submitted identity matches the actual pending response, transaction `receipts_persisted`), plus the `post-error` control (HTTP 500 → cleanup DELETE 204, transaction `cleaned_up` with `cleanup: succeeded`, strict exit nonzero) | deterministic scripted |
| 7 | Correct final review payload, bounded output, revision identity | payload: public finding present, private audit sentinel absent from the public payload and transport log, bounded inline budget (`comments: []`); bounded output: exact per-scenario provider-call denominators; revision identity: `input/revision-admission.json` binds `candidate_head` to the fixture's exact commit OID with a sha256-width identity digest; negative: mismatched-identity control (foreign review id 988) + `tests/release_result_planes.rs::advisory_result_planes_cannot_override_legacy_gate_enforcement` (existing main test, mapped not duplicated) | deterministic scripted |

Live hosted facets (cases 1–2): the documented Action-smoke dispatch
(`gh workflow run action-smoke.yml` with `run_model_smoke=true`, bounded
defaults: lane width 10, max 14 model calls) marks live provider use as an
explicit opt-in. This PR ships the scaffolding that consumes that run's
uploaded `ub-review-model-smoke` receipt — set `UB_MODEL_SMOKE_RECEIPTS` to
the extracted artifact directory and run
`cargo test --locked --test archive_advisory_run
hosted_model_smoke_receipt_qualifies_live_advisory_conclusion -- --ignored`.
The consumer requires a usable `pass` conclusion, ≥N usable ok/degraded
lanes counted with the repo verifier's denominator (refuter lanes excluded),
and every lane/preflight on the approved provider of the documented dispatch
(`minimax`); its negative controls above reject inconclusive or failing
conclusions, unapproved-provider lanes, unapproved-provider preflights,
too-few usable lanes, and refuter-only usable counts. The orchestrator
dispatches the hosted run; no live provider call is attempted locally.

The documented dispatch runs `provider-policy: minimax-only`, so a live
fallback (primary outage completing on the fallback provider) is not
reachable through it; case 2 is qualified by the deterministic scripted
scenario, and this boundary is a known gap, recorded below.

## Execution boundaries

The harness gives every child a cleared environment with an allowlisted
`PATH` (`git`, `uname`, `sh` symlinks plus the synthetic adapter), `HOME` and
`LC_ALL=C`, file-backed logs and a 30-second direct-child kill/reap boundary —
not a general process-tree containment claim. The only curl implementation is
an inert synthetic adapter with exact `.invalid` URL routes; each route first
demands the synthetic fixture credential of its endpoint family
(`X-Api-Key` for the MiniMax anthropic route, `Authorization: Bearer` for the
OpenCode route), unknown routes exit without networking, and provider or
GitHub credentials are synthetic. The adapter's transport log is hashed into
the per-scenario record, and the exact per-scenario provider-call denominator
is pinned (model-off/required-gap/missing-key: zero calls; outage: preflight
plus one provider attempt, both refused 503; fallback: primary preflight 503,
fallback preflight, one lane call; normal: preflight plus one lane call).

The fixture repository is a conventional Rust workspace with one deterministic
change (`41` → `42`). Git revision/diff commands must exit successfully before
their output becomes input evidence;
`campaign::fixture_identity_and_diff_fail_closed_on_real_git_errors` runs all
three operations against a real non-repository. Fixture commands run under the
same child quarantine rules as production (#1650): the binary's sensor and
proof children inherit only the allowlisted environment, and the fixture
config disables every builtin sensor so the campaign observes model and
delivery behavior alone. If a builtin tool is added without extending the
fixture disable list, its missing command surfaces as sensor evidence and the
pinned result planes fail loudly — the list is a discriminator, not a
documentation exercise.

`receipt.json` (`ub-review.archive_advisory_campaign.v2`) records the
candidate binary digest and version, harness and transport-adapter digests,
every command (argv, environment minus credential names, exit code,
stdout/stderr hashes), and per-scenario planes, transport hashes, packet
digests and revision-admission digests. A wrong expected conclusion must
reject every emitted run result (per-scenario wrong-outcome control), the
emitted structured packet must pass the ordinary verifier, and a deliberately
corrupted copy must fail with exactly the gate-schema denial
(`campaign::oracle::known_packet_denial_cannot_accept_success_or_unrelated_failures`
pins that predicate against success, unrelated denials, extra denials and
chatter).

## Pinned controls on current main

- Observed and asserted result planes (all seven packets verified by the
  ordinary packet verifier; a deliberately corrupted copy of the structured
  packet is rejected with exactly the gate-schema denial; every scenario also
  rejects a flipped conclusion and pins its terminal state in both
  `gate_outcome.terminal_status` and `review/terminal_state.json`):
  - `structured-finding`, `provider-fallback`: conclusion `pass`, gate
    `pass`, analysis `findings`, publication `posted`, terminal
    `needs-reviewer-attention`. The `posted` label is the producer's
    prepare-time label (#959): the run labels it from payload preparation
    alone, and in this campaign the label is backed by a real confirmed
    in-packet post with an `ok` receipt.
  - `provider-outage`, `missing-key`: conclusion `pass`, gate
    `not_proven`, analysis `not_proven`, publication `not_proven`; terminal
    state `failed-to-review`.
  - `provider-text`: conclusion `pass`, gate `pass`, analysis `limited`,
    publication `not_needed`; terminal `sufficient`; the degraded lane is
    retained.
  - `model-off`: conclusion `pass`, gate `not_proven`, analysis
    `not_proven`, publication `not_needed`; terminal state `artifact-only`.
  - `required-gap`: conclusion `inconclusive`, gate `not_proven`, analysis
    `not_proven`, publication `not_proven`; terminal `failed-to-review`; the
    gate `reasons[]` must contain the `required-sensor` reason with
    `id: tokmd` (`disabled by config`), asserted so the inconclusive
    conclusion cannot drift to a different cause.
- Provider denominators (asserted, with the outage rows additionally checked
  for POST-to-provider + 503 composition): model-off / required-gap /
  missing-key make zero transport calls; the outage makes exactly two calls
  (preflight plus one provider attempt, both refused 503 on the MiniMax
  route); the fallback makes exactly three (MiniMax preflight 503, OpenCode
  preflight ok, one lane call on OpenCode); a normal structured review makes
  exactly two (preflight plus one lane call).
- Case 2 discriminator: the fallback scenario's lane receipt names provider
  `opencode-go`, status `ok`, with `fallback_from` naming `minimax`.
- The post-error receipt on current main reports `http_status: 500`,
  `error_kind: post_http_error`, `failure_stage: network_post` for a failed
  submit — the retained branch observed `failure_stage: unknown` with a null
  status; current main classifies the stage, and this campaign pins the
  current observation. The failed submit leaves the transaction
  `cleaned_up` with `cleanup: succeeded` (adapter rows: POST
  `/reviews/987/events` → 500, DELETE `/reviews/987` → 204), and the strict
  post exits nonzero.
- The mismatched-identity control records that the CLI still accepts a
  submitted response carrying a foreign review id (988 instead of the pending
  987); the campaign identity oracle refuses to count that acceptance as
  identity-negative evidence unless the specimen is exactly the mismatched
  submitted state. Production submission identity authority remains #1317.
- The structured-finding prepared payload is checked before any synthetic
  post and carries no `post-result.json` at that point: a prepared payload
  is not, by itself, delivery confirmation.
- Every emitted packet's `input/revision-admission.json` binds semantics
  `candidate_head` and `reviewed_commit_oid` to the fixture's exact head
  object with a sha256-width `identity_digest` (case 7 revision identity).

## Known gaps

- Live hosted facets await the orchestrator's Action-smoke dispatch; until it
  runs, case 1's live half and the hosted receipt consumer are unexercised
  against real provider output.
- A live provider fallback cannot be induced through the documented
  minimax-only dispatch; case 2 rests on the deterministic scripted scenario.
- The campaign qualifies candidate source behavior on Linux x86_64 only;
  the platform matrix and final archive bytes/provenance remain with their
  owners (#816, retained packaging runbook).
- The mismatched-identity acceptance is an observed producer boundary, not a
  qualification of it.

## Local evidence, 2026-11-06

Candidate content: main `9b0515bb` plus this PR's test/doc additions; the
campaign drives the source-built binary (`CARGO_BIN_EXE_ub-review`), so every
observation qualifies candidate content directly.

| Evidence | Command | Result |
|---|---|---|
| Windows formatting | `cargo fmt --all` | clean |
| Windows lint | `cargo clippy --locked --workspace --all-targets` (private `F:/Temp/fix-1341-target`) | exit 0, no warnings |
| Windows oracle tests | `cargo test --locked --test archive_advisory_run` | 5 passed, 1 ignored (live consumer pending hosted receipt) |
| Linux lint | `cargo clippy --locked --workspace --all-targets` (Ubuntu WSL, cargo 1.95.0, private `~/fix-1341-wsl-target`) | campaign file clean; two pre-existing `unused import: Context` warnings on untouched `src/proof_task_ledger.rs` and `src/sensors/mod.rs` test modules exist on main and are out of this diff's scope |
| Linux campaign | `cargo test --locked --test archive_advisory_run` (Ubuntu WSL, Linux x86_64) | 7 passed, 1 ignored, 0 failed; campaign ~63s: seven run scenarios plus the post-error and mismatched-identity controls, all packets verifier-verified |

These local runs are not hosted release evidence. The hosted action-smoke
dispatch (`run_model_smoke=true`) and its receipt consumption remain the
orchestrator's step.
