# Action and CLI reference

The [GitHub Action definition](../action.yml) is the source of truth for the full input and output list, defaults, and accepted values. This page keeps the useful operating boundaries and examples out of the [README](../README.md).

## Primary modes

| Setting | Purpose |
| --- | --- |
| `review-mode: advisory` | Nonblocking review posture. This does not isolate untrusted code or remove credentials by itself. |
| `mode: review-byok` | Grouped BYOK review (default compatibility mode). |
| `mode: intelligent-ci` | CI evidence mode; not proof that the intended sole-gate architecture is qualified. |
| `model-mode: off` | No model investigation. A run with this setting cannot claim a semantic model review. |
| `posting: artifact-only` | Retain evidence without submitting a GitHub PR review. |
| `posting: review` | Prepare and attempt one grouped Pull Request Review when configured. |
| `allow-heavy: false` | Do not authorize heavy build/test/Miri/mutation witnesses by default. |
| `install-mode: auto` | Tagged Action refs prefer the release archive and may fall back to source; `release` is strict. |

Use [advisory adoption](ADOPTION_ADVISORY.md) before granting model or posting credentials. The Action has distinct `review-mode` and legacy `mode` inputs; keep their effective behavior and the [current authority boundary](PRODUCT_STATE.md) in view.

## Sensors and proof

The default core sensor bundle is best-effort:

- `tokmd` prepares deterministic repository, diff, and LLM context.
- `cargo-allow` records source exception-policy drift.
- `ripr` identifies weak changed-behavior test oracles.
- `unsafe-review` examines the review evidence around changed unsafe Rust.
- `ast-grep` performs structural source-route scans.
- `actionlint` inspects workflow changes.

A missing selected sensor is missing evidence, not a clean sensor result. A required tool applies only when its trigger matches the diff. Repository configurations can require applicable tools:

```toml
[tools.actionlint]
required = true
```

The Rust unsafe evidence stack distinguishes exception-policy drift (`cargo-allow`), weak test oracles (`ripr`), unsafe-contract reviewability (`unsafe-review`), concrete mutation-test failure (`cargo-mutants`), concrete UB witnessed by execution (Miri), and execution coverage (Codecov). These do not establish the same proposition. See [unsafe-review policy](UNSAFE_REVIEW_POLICY.md) and [CI guidance](ci/unsafe-review.md).

A repository can also declare approved Required proof for intelligent CI, subject to the central allowlist and runtime budget:

```toml
review_profile = "bun-ub-v0"
profile = "gh-runner"

[repo]
kind = "rust"

[[proof.required]]
id = "cargo-check"
languages = ["rust"]
diff_classes = ["source-general", "source-ub"]
command = "cargo check --workspace --locked"
reason = "Required Rust workspace check for intelligent CI."
cost = "focused-build"
timeout_sec = 300
required = true
```

The example above is a proof-policy configuration, not a blanket recommendation to run arbitrary commands from an untrusted PR.

## CLI operations

| Command | Job |
| --- | --- |
| `ub-review init --profile gh-runner` | Generate starter configuration, **instead of** `enable`. |
| `ub-review enable --inspect --mode advisory --model minimax` | Write an advisory workflow/config in a new setup. `--inspect` is not read-only. |
| `ub-review audit-ci` | Inventory existing CI and write local audit receipts without changing CI or protection. |
| `ub-review setup-ci --print-pr` | Plan a separate CI migration; without accepted jobs it writes `migration-plan.md` only. |
| `ub-review run` | Prepare the review and evidence packet. |
| `ub-review post` | Submit a previously prepared grouped review payload. |
| `ub-review gate-check` | Enforce the recorded compatibility gate conclusion under the selected policy. |

To prepare and submit a review explicitly:

```bash
ub-review run --posting review --out target/ub-review
ub-review post --review-json target/ub-review/review/github-review.json
```

The post step has its own success/error receipts. Current `publication-result=posted` may mean a review payload was prepared before GitHub confirmed delivery. Don't infer delivered comments from that value alone.

For a CI migration that actually renders proposed new files, `setup-ci --print-pr` requires at least one audited job with a maintainer-supplied command and a reviewed Action commit:

```bash
ub-review setup-ci --print-pr \
  --accept "$AUDITED_JOB=$REPOSITORY_OWNED_COMMAND" \
  --action-sha "$REVIEWED_UB_REVIEW_SHA"
```

`setup-ci --open-pr` requires those inputs and GitHub authentication. Neither command changes branch protection. See [Getting started](QUICKSTART.md).

## Outputs and authority

| Output | Meaning |
| --- | --- |
| `out`, `summary-path`, `events-path` | Packet directory, running summary, append-only events. |
| `review-json-path`, `metrics-json-path` | Private review and quality/cost metrics. |
| `github-review-path` | Prepared grouped PR-review payload. |
| `post-result-path`, `post-error-path`, `post-payload-path`, `post-stdout-path`, `post-stderr-path` | Exact attempted delivery and response receipts. |
| `gate-outcome-path` | The run-stage gate outcome artifact. |
| `gate-conclusion` | Legacy enforced compatibility value: `pass`, `fail`, or `inconclusive`. |
| `analysis-result` | `clean`, `findings`, `limited`, or `not_proven`. |
| `publication-result` | `posted`, `not_needed`, `failed`, or `not_proven`; prepared output is not confirmed delivery. |
| `gate-result` | Additive evidence report: `pass`, `finding`, or `not_proven`. Not the current enforcement field. |
| `not-proven-reasons`, `sensor-coverage` | JSON evidence-gap reasons and coverage counts (parse with `fromJSON`). |

`gate-conclusion` remains enforcement authority on the current Action. The intended finalized CI result is separately specified in [Product state](PRODUCT_STATE.md#ci-outcome-vocabulary). In particular, additive `gate-result: finding` is not automatically finalized `fail`.

## Installing the Action

With `install-mode=auto`, tagged refs prefer a Linux x64 release archive and may fall back to building from source. Commit SHA refs use source installation. Explicit `install-mode=release` refuses a missing archive, missing or mismatched checksum, or unsupported runner instead of silently rebuilding.

Separate the Action SHA, release artifact, sensor bundle, and reviewed candidate identity. The full supported options and defaults live in [`action.yml`](../action.yml); [Bun integration](GH_RUNNER_BUN.md) covers its specialist preset.
