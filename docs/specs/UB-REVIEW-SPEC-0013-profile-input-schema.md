# UB-REVIEW-SPEC-0013 — profile / config input schema

Status: authored 2026-06-22 (Wave 6+, docs-only).
Umbrella: [UB-REVIEW-SPEC-0001](UB-REVIEW-SPEC-0001-release-surface.md).
Related: [SPEC-0002](UB-REVIEW-SPEC-0002-review-byok.md) (providers),
[SPEC-0006](UB-REVIEW-SPEC-0006-provider-cache-fallback.md) (provider keys),
[SPEC-0009](UB-REVIEW-SPEC-0009-bun-ub-preset.md) (preset profile),
[SPEC-0011](UB-REVIEW-SPEC-0011-lane-doctrine.md) (lanes).
Maturity: production — the schema is the load-bearing config surface every
consumer writes; this spec makes the live/inert/echo distinction explicit so
consumers do not rely on keys that parse but drive nothing.

## Purpose

Name the input TOML schema for ub-review config files (`.ub-review.toml`,
`profiles/*.toml`, `runtime/*.toml`): which keys are accepted, their types,
their liveness (LIVE / ECHO-ONLY / INERT), and how review-profile, runtime-
profile, user config, and CLI overrides compose. SPEC-0009 references profile
files; no spec previously owned which keys are accepted vs reserved/inert —
the inert-key problem (#609 / UB-26) lives here.

## Liveness definitions

| Class | Meaning |
|---|---|
| **LIVE** | Consumed by non-test code outside `config.rs`; drives behavior. |
| **ECHO-ONLY** | Propagates into an artifact (`resolved-plan.json`, `resolved-profile.json`, etc.) but never branches behavior. |
| **INERT** | Parsed and stored but only referenced in `config.rs` itself or test asserts. Drives nothing. |

> **Consumers should not rely on INERT or ECHO-ONLY keys for behavior.** They
> are either documentation of intent (may be wired in future) or vestigial.
> The config validator produces `PolicyError` receipts for *unknown* keys, but
> known keys that are inert parse silently — this spec is the authoritative
> list of which is which.

## Top-level config keys (`Config`)

| Key | Type | Default | Liveness |
|---|---|---|---|
| `review_profile` | `String` | `"bun-ub-v0"` | ECHO-ONLY |
| `profile` | `String` | `"gh-runner"` | LIVE |
| `repo` | table | see below | mixed |
| `review` | table | see below | mixed |
| `review_body` | table | see below | LIVE (all fields) |
| `gate` | table | see below | mixed |
| `proof` | table | see below | LIVE |
| `profiles` | map of runtime profiles | builtin set | LIVE |
| `tools` | map of tool policies | builtin set | LIVE |
| `lanes` | array of `[[lanes]]` | `[]` | LIVE |
| `issues` | table | see below | LIVE (all fields) |
| `providers` | table | see below | LIVE (subset) |

Unknown top-level keys → `PolicyError` receipt.

## `[repo]`

| Key | Type | Default | Liveness |
|---|---|---|---|
| `kind` | `String` | `"bun"` | ECHO-ONLY (propagates to `repo_kind` artifact key; free-text, not validated) |
| `ledger` | `String` | `""` | LIVE (read as bounded review context) |
| `base` | `String` | `"origin/main"` | ECHO-ONLY (diff context comes from `DiffContext`, not this field) |
| `head` | `String` | `"HEAD"` | ECHO-ONLY |

## `[review]`

| Key | Type | Default | Liveness |
|---|---|---|---|
| `posting_engine` | `String` | `"github-step-summary"` | ECHO-ONLY |
| `custom_poster` | `bool` | `false` | **INERT** |
| `ban_standalone_approval` | `bool` | `true` | **INERT** (reads as enforced posture but enforces nothing) |
| `require_zero_finding_audit` | `bool` | `true` | **INERT** (reads as enforced posture but enforces nothing) |
| `enable_default_lanes` | `bool` | `true` | LIVE |
| `github_summary` | `bool` | `true` | LIVE |

> The two INERT `review.*` keys (`ban_standalone_approval`,
> `require_zero_finding_audit`) are the most dangerous inert keys — their
> names imply security-relevant posture. See #609 for the wire/document/remove
> decision per key.

## `[review_body]` — all LIVE

| Key | Type | Default | Values |
|---|---|---|---|
| `include_successful_lane_table` | `bool` | `false` | — |
| `include_provider_table` | `never \| on_failure \| always` | `on_failure` | — |
| `include_sensor_table` | `never \| on_failure \| always` | `on_failure` | — |
| `include_execution_summary` | `none \| on_failure \| always` | `none` | — |
| `summary_only_body` | `suppress \| post_substantive \| post_all` | `suppress` | controls what the PR body contains |

## `[gate]` and `[gate.blocking]`

| Key | Type | Default | Liveness |
|---|---|---|---|
| `gate.required_check` | `String` | `"ub-review/gate"` | LIVE |
| `gate.target_minutes` | `u64` | `30` | LIVE (drives `floor_budget_pressure_detected`) |
| `gate.hard_timeout_minutes` | `u64` | `60` | ECHO-ONLY (cost receipt `cap_minutes` only) |
| `gate.post_review_on` | `Vec<String>` | `["opened","ready_for_review"]` | LIVE |
| `gate.blocking.required_proof_unproven` | `bool` | `false` | LIVE |
| `gate.blocking.tool_gate_missing_evidence` | `bool` | `false` | LIVE |

`[gate]` and `[gate.blocking]` carry `deny_unknown_fields`. The legacy
`gate.synchronize_mode` key produces a hard-coded deprecation receipt.

## `[[proof.required]]` — all LIVE

Each entry (`deny_unknown_fields`):

| Key | Type | Default |
|---|---|---|
| `id` | `String` | `""` |
| `languages` | `Vec<String>` | `[]` (wildcards: `*`, `any`, `all`) |
| `diff_classes` | `Vec<String>` | `[]` (wildcards: `*`, `any`, `all`) |
| `command` | `String` | `""` |
| `reason` | `String` | `""` |
| `cost` | `Option<String>` | `None` (`focused-test \| focused-build \| manual`) |
| `timeout_sec` | `u64` | `300` |
| `required` | `bool` | `true` |
| `enabled` | `bool` | `true` |

`[proof]` accepts ONLY `required`; any other `[proof.X]` is a receipt. See
SPEC-0012 for the broker's allowlist (only `focused-test` and `focused-build`
costs are brokerable; `manual` is never executed).

## `[tools.<id>]` — all LIVE

Built via custom `Deserialize` that records which fields were supplied
(`ToolPolicyProvided`), so `merge_defaults` gap-fills only absent fields.
User-supplied fields (including zeros) survive the merge.

| Key | Type | Default |
|---|---|---|
| `id` | `String` | `""` |
| `command` | `String` | `""` |
| `class` | `packet \| static \| search \| workflow \| security \| coverage \| test \| build \| heavy-witness` | `static` |
| `weight` | `u32` | `1` |
| `default` | `always \| source-changed \| ... \| never` | `never` |
| `required` | `bool` | `false` |
| `timeout_sec` | `u64` | `120` |
| `artifact_budget_mb` | `u64` | `64` |
| `requires_lease` | `bool` | `false` |
| `enabled` | `bool` | `true` |
| `gate` | table (see below) | `None` |

`[tools.<id>.gate]`:

| Key | Type | Default | Liveness |
|---|---|---|---|
| `scope` | `Option<String>` | `None` | ECHO-ONLY (only `on-diff` accepted; validated then never branched) |
| `max_new_unsuppressed` | `Option<u64>` | `None` | LIVE |

## `[[lanes]]` — all LIVE

| Key | Type | Default |
|---|---|---|
| `id` | `String` | `""` |
| `role` | `String` | `""` |
| `focus` | `String` | `""` |
| `receives` | `Vec<String>` | `["tokmd","ripr","ast-grep"]` |
| `model` | `String` | `""` (empty → lane default) |
| `diff_classes` | `Vec<String>` | `[]` (empty → `"all"`) |

See SPEC-0011 for the lane-doctrine contract. Entries with empty `id`/`focus`
are skipped with a plan note (non-fatal).

## `[issues]` — all LIVE

| Key | Type | Default |
|---|---|---|
| `enabled` | `bool` | `true` |
| `mode` | `String` | `"suggest"` (`off \| suggest \| open-high-confidence`) |
| `open_in` | `Vec<String>` | `[]` (allowlist of `owner/repo` slugs, no wildcards) |
| `open_cap` | `u32` | `3` |

## `[providers]`

| Key | Type | Default | Liveness |
|---|---|---|---|
| `policy` | `String` | `""` | LIVE (`auto \| minimax-primary \| primary-with-fallback \| minimax-only \| opencode-go-canary \| opencode-go-wide`) |
| `minimax.max_concurrency` | `Option<usize>` | `None` | LIVE |
| `minimax.prompt_cache` | `Option<String>` | `None` | LIVE (`explicit-anthropic \| off`) |
| `opencode.max_concurrency` | `Option<usize>` | `None` | LIVE |
| `opencode.prompt_cache` | — | — | rejected (no implementation) |
| `*.env`, `*.model`, `*.role`, `*.models` | — | — | **INERT** (tolerated/stripped; documentation of intent only — see SPEC-0006) |

## Runtime profile schema (`runtime/*.toml`)

A runtime `Profile` has:

```
name          : String
limits        : Limits      (13 usize fields)
guards        : Guards      (3 fields)
budgets       : Budgets     (15 fields)
trusted_repo  : TrustedRepo (3 fields — ALL INERT)
tool_timeouts : BTreeMap<String, u64>  (per-tool-id override; LIVE)
```

### `[limits]`

| Key | Default | Liveness |
|---|---|---|
| `llm_in_flight` | `16` | LIVE |
| `sensor_jobs` | `4` | LIVE |
| `tests` | `2` | LIVE |
| `builds` | `0` | LIVE |
| `logical_lanes` | `20` | **INERT** |
| `repo_read` | `6` | **INERT** |
| `raw_file_reads` | `6` | **INERT** |
| `grep` | `3` | **INERT** |
| `ast_grep` | `2` | **INERT** |
| `git` | `2` | **INERT** |
| `rust_analyzer` | `0` | **INERT** |
| `summary_writers` | `1` | **INERT** |
| `patch_writers` | `0` | **INERT** |

### `[guards]` — all LIVE

| Key | Type | Default |
|---|---|---|
| `min_free_mem_mb` | `u64` | `1500` |
| `min_free_disk_mb` | `u64` | `4000` |
| `max_load_1m` | `f32` | `6.0` |

### `[budgets]` — all LIVE

| Key | Type | Default |
|---|---|---|
| `artifact_budget_mb` | `u64` | `750` |
| `scratch_budget_mb` | `u64` | `4000` |
| `default_timeout_sec` | `u64` | `1800` |
| `hard_timeout_sec` | `u64` | `3600` |
| `proof_max_focused_test_files` | `usize` | `3` |
| `proof_max_focused_tests` | `usize` | `1` |
| `proof_command_timeout_sec` | `u64` | `300` |
| `proof_total_timeout_sec` | `u64` | `600` |
| `proof_cpu` | `u32` | `2` |
| `proof_memory_mb` | `u64` | `2048` |
| `proof_disk_mb` | `u64` | `1024` |
| `proof_network` | `bool` | `false` |
| `proof_scratch` | `bool` | `true` |
| `mutation` | `bool` | `false` |
| `sanitizer` | `bool` | `false` |

See SPEC-0012 for how `proof_*` budget fields constrain the broker.

### `[trusted_repo]` — ALL INERT

| Key | Type | Default | Liveness |
|---|---|---|---|
| `pass_triggers` | `Vec<String>` | `["opened","ready_for_review"]` | **INERT** |
| `synchronize` | `bool` | `false` | **INERT** |
| `proof_lanes` | `Vec<String>` | `["focused-tests",...]` | **INERT** |

> `trusted_repo` is entirely inert — all three fields drive nothing. The doc
> comment on `Profile` suggests these *should* drive the trusted-repo gate
> path, but they currently do not. Flagged for #609.

## Merge / override precedence

`Config::load_or_default` composes in this order (later wins):

1. `Config::default()` — seeds `profiles` from `builtin_profiles()`, `tools`
   from `builtin_tools()`, and hard-coded defaults for every scalar section.
2. User TOML — parsed via `from_toml_with_policy_receipts` →
   `sanitize_policy_sections`. User-provided keys replace defaults
   field-by-field.
3. CLI `--profile` / `--runtime-profile` override — wins over both the TOML
   `profile` key and the default.
4. `merge_defaults()` — re-introduces any missing builtin profile and
   **gap-fills** every `[tools.<id>]` using `ToolPolicyProvided` flags. Only
   fields the user did NOT supply are filled from builtin defaults. User-
   supplied fields (including zeros) survive; `merge_defaults` never
   overwrites a user-supplied tool field.
5. `auto` resolution — if the resolved profile is `"auto"`,
   `BoxState::detect()?.suggested_profile()` replaces it.
6. Unknown-profile receipt — if the final profile name is not in `profiles`
   (and isn't `"gh-runner"`), a `policy_errors` entry is pushed (#608) and
   `selected_profile()` falls back to `"gh-runner"` at read time.

**Precedence (highest to lowest):** CLI `--profile` > `auto` detection > TOML
`profile` key > default `"gh-runner"`. For tool fields: **user TOML > builtin
defaults** (merge_defaults only gap-fills).

## Known-key allowlists

| Constant | Accepted values |
|---|---|
| Top-level keys | `review_profile, profile, repo, review, review_body, gate, proof, profiles, tools, lanes, issues, providers` |
| `[tools.<id>]` keys | `id, command, class, weight, default, required, timeout_sec, artifact_budget_mb, requires_lease, enabled, gate` |
| `[tools.<id>.gate] scope` | `on-diff` (the only valid value) |
| `languages` selectors | `rust, typescript, javascript, c-cpp, zig, go, python, shell, yaml, toml, json, markdown, mixed` |
| `diff_classes` selectors | `source-ub, source-general, tests-only, workflow/tooling, docs-only, artifact-only-smoke` |
| Wildcards | `*, any, all` (match any class/language) |

Selector matching normalizes: trim → ASCII-lowercase → `_` → `-`. So `c_cpp`
is accepted as `c-cpp`.

## Verification

The liveness classifications above are derived from grep of the codebase at
HEAD. The load-bearing validations are test-pinned:

- Unknown-key stripping: `unknown_top_level_keys_produce_policy_error_receipts`,
  `policy_parse_errors_are_recorded_receipts_not_silent_defaults`.
- Tool gap-fill: `merge_defaults_preserves_user_supplied_tool_zero_over_default`.
- Profile fallback receipt (#608):
  `unknown_profile_override_records_policy_error_not_silent_fallback`.
- Example config parse: `ub_review_example_config_loads_clean_and_demonstrates_schema`,
  `unsafe_review_swarm_recommended_config_loads_advisory_floor`.

## Inert-key disposition

This spec documents the inert keys; the decision to wire, document-as-inert,
or remove each is tracked in #609. Until those decisions land, the
classifications here are the authoritative reference for what a consumer can
rely on.
