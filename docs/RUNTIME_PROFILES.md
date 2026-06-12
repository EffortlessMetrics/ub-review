# Runtime profiles

Runtime profiles keep implementation names plain and technical:

- `gh-runner`
- `gh-runner-standard`
- `gh-runner-full`
- `cx23`
- `cx33`
- `cx43`

A runtime profile is a resource lease preset, not a product tier. It defines
how much local work ub-review may centrally schedule while preparing evidence
for a PR review. `gh-runner` is the zero-config compatibility alias for
`gh-runner-standard`.

Runner profile is part of the product surface. GitHub-hosted runners are the
boring adoption default and are suitable for quick, normal, and advisory runs.
Merge-critical stacks, long proof, and heavy optional fills should use a `cx*`
or other self-hosted profile once hosted-runner cancellation or eviction starts
to obscure the code signal. A cancelled hosted run is infrastructure evidence,
not a code failure; audit-ci records that diagnosis in
`ci-audit/runner-cancellations.json`.

## Shared trusted-repository defaults

Trusted repositories use two passes per PR by default:

- `opened`
- `ready_for_review`

There is no default `synchronize` trigger.

Each pass targets 30 minutes of local proof work and has a hard timeout of 60 minutes. Model calls are network I/O scheduled concurrently with local commands; provider wait does not reserve the CPU, disk, or local proof budget, and the pass still obeys the runtime timeout. All runtime profiles keep the PR body pure signal and place command logs, lane outputs, model status, resource leases, metrics, raw observations, and proof stdout/stderr in artifacts.

## Profile intent

| Profile | Intended runner | Local proof posture |
|---|---|---|
| `gh-runner` | GitHub-hosted runner | Compatibility alias for `gh-runner-standard`; use this when adopting the action with no extra configuration. |
| `gh-runner-standard` | GitHub-hosted runner | Focused tests, base+tests red/green, actionlint, scoped source-route checks, and lightweight proof. |
| `gh-runner-full` | Larger or explicitly leased GitHub runner | Standard proof plus leased heavy witnesses such as targeted mutation or sanitizer work when relevant. |
| `cx23` | Small local coordinator | Minimal local execution, high selectivity, and model reasoning over prepared evidence. |
| `cx33` | Balanced local runner | Full fast sensors and focused proof with moderate leases. |
| `cx43` | Stronger local runner | Wider sensor fanout, more local tests, and occasional leased build/heavy-witness work. |

## Per-tool Sensor Leases

A runtime profile may include a `[tool_timeouts]` table mapping tool ids to
sensor lease seconds. This lets the runner profile express what the box can
afford without making every repo copy the same tool override.

Resolution order is:

1. explicit repo config `[tools.<id>] timeout_sec`;
2. runtime profile `[tool_timeouts]`;
3. built-in tool default.

The resolved value is still capped by the profile's
`budgets.default_timeout_sec`. The GitHub runner profiles ship `ripr = 720`;
the local `cx*` profiles keep the built-in one-size sensor leases.

## Resource rule

A lane requests proof; it does not own the runner. The proof-planning lane reads the diff, sensor output, early lane observations, repository configuration, and available receipts, then recommends the smallest proof set that can change the review decision. The orchestrator ranks and routes those requests, the proof broker runs the commands while model lanes continue over the network, and the resource broker enforces the local lease. Work is eligible only when it is relevant to the PR, centrally scheduled, deduped, budgeted, leased, receipted, and likely to change the review decision.
