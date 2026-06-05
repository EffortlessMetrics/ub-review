# Porting Baseline

Use this guide when adopting the `ub-review` operating baseline in another
serious Rust repo. It is a support package, not an engine spec: configure the
repo to emit evidence, keep exceptions owned, and let the review gate post only
decision-changing text.

## Choose The Mode

Start with one of two modes:

| Mode | Use when | Posting |
|---|---|---|
| `review-byok` | the repo wants one grouped review over a shared packet | optional PR review |
| `intelligent-ci` | the repo wants `ub-review` as a required evidence gate | gate status plus optional PR review |

Use `review-byok` for first adoption. Move to `intelligent-ci` only after the
artifact verifier is green and the repo knows which tools are mandatory.

## Minimum Files

Add these files first:

```text
AGENTS.md
docs/REPO_STYLE.md
docs/REVIEW_BODY_CONTRACT.md
docs/RUNNER_IMAGE.md
docs/PORTING_BASELINE.md
policy/allow.toml
policy/ci-budget.toml
policy/ci-lanes.toml
policy/ci-risk-packs.toml
```

Use this repo's files as templates, then delete policy that the target repo
does not own yet. A small crate should not pretend to have an industrial runner
image; record missing tools as missing evidence until the image exists.

## Tool Stack

Keep tool roles separate:

| Tool | Porting role |
|---|---|
| `cargo-allow` | source-tree exception ledger |
| `ripr` | static mutation-exposure / weak-oracle signal |
| `unsafe-review` | unsafe/native safety contract reviewability |
| `ast-grep` | cheap structural sibling scans |
| `actionlint` | workflow diffs |
| coverage / Codecov | execution-surface telemetry |
| cargo-mutants / Miri / ASAN | scoped runtime backstops |
| `xtask` | repo-local orchestration and receipt normalization |

Do not reimplement these tools inside `xtask` or `ub-review`. Call them,
normalize receipts, and file grounded tool defects in the matching upstream
repo.

## Required Tool Behavior

Default behavior:

- generic hosted runner missing a tool: evidence gap;
- standard image missing a core tool: image drift;
- successful tool with no decision-changing result: artifact only;
- missing coverage: unknown execution-surface telemetry, not test failure;
- high coverage: execution signal, not correctness proof.

When a repo makes a tool required, scope the requirement to the matching diff.
For example, actionlint should not block a source-only PR.

```toml
[tools.actionlint]
enabled = true
required = true
default = "workflow-changed"
```

## Review-Fast PR Rule

The target repo should teach agents to open PRs with one proof obligation:

```text
one seam
one claim boundary
one route map
one focused proof or explicit witness gap
parked sibling paths
```

Review-fast does not mean tiny. It means the reviewer can decide the claim from
the packet without reconstructing the investigation.

## Artifact Contract

Each run should upload the full packet:

```text
target/ub-review/
  input/
  sensors/
  lanes/
  review/
    review.json
    review.md
    terminal_state.json
    metrics.json
    proof_requests.json
    proof_receipts.json
    resource_leases.json
    github-review.json        # only when posting reviewer-value content
    github-review-skip.json   # when artifact-only is correct
  events.ndjson
  work_queue.json
  work_events.ndjson
  running-summary.md
```

For a copied packet, verify with:

```bash
python scripts/verify-bun-review-artifacts.py target/ub-review
```

Use stricter flags only after the target repo has stable model lanes and sensor
availability.

## PR Body Contract

Reviewer-facing text is not an audit log. It may contain:

- decision;
- findings;
- verification questions;
- proof results;
- refutations;
- parked follow-ups;
- specific evidence gaps that change trust.

It must not contain setup tables, lane rosters, provider tables, command logs,
generic no-finding prose, or tool chatter that cannot change the current PR's
decision. Full receipts stay in artifacts.

## First Adoption PR

The first PR in a new repo should be boring:

1. Add the policy/docs baseline.
2. Add the workflow in artifact-only posting mode.
3. Run the packet on a draft PR.
4. Download the packet and run the verifier.
5. Record missing tools and model lanes as known gaps.
6. Only then enable PR review posting or required-gate status.

Do not claim the repo is protected until the uploaded packet verifies and the
workflow pin points to a known-good `ub-review` SHA or release tag.

## Validation Before Merge

For baseline changes in this repo, prefer:

```bash
cargo fmt --all -- --check
cargo test --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo xtask policy-check
python scripts/verify-bun-review-artifacts.py --self-test
git diff --check
```

For docs-only porting updates, `cargo xtask policy-check` and `git diff
--check` are the minimum local checks. Record any skipped tool, such as missing
local `actionlint`, as a validation gap.

## Handoff

When the target repo exposes a tool problem, file the defect with:

- command;
- repo/ref;
- artifact excerpt;
- expected behavior;
- actual behavior;
- impact on review or UB proof;
- acceptance criteria.

Use [SENSOR_ROUTING.md](SENSOR_ROUTING.md) for the current tool-owner map.
