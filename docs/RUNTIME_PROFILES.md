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

## Shared trusted-repository defaults

Trusted repositories use two passes per PR by default:

- `opened`
- `ready_for_review`

There is no default `synchronize` trigger.

Each pass targets 30 minutes and has a hard timeout of 60 minutes. All runtime profiles keep the PR body pure signal and place command logs, lane outputs, model status, resource leases, metrics, raw observations, and proof stdout/stderr in artifacts.

## Profile intent

| Profile | Intended runner | Local proof posture |
|---|---|---|
| `gh-runner` | GitHub-hosted runner | Compatibility alias for `gh-runner-standard`; use this when adopting the action with no extra configuration. |
| `gh-runner-standard` | GitHub-hosted runner | Focused tests, base+tests red/green, actionlint, scoped source-route checks, and lightweight proof. |
| `gh-runner-full` | Larger or explicitly leased GitHub runner | Standard proof plus leased heavy witnesses such as targeted mutation or sanitizer work when relevant. |
| `cx23` | Small local coordinator | Minimal local execution, high selectivity, and model reasoning over prepared evidence. |
| `cx33` | Balanced local runner | Full fast sensors and focused proof with moderate leases. |
| `cx43` | Stronger local runner | Wider sensor fanout, more local tests, and occasional leased build/heavy-witness work. |

## Resource rule

A lane requests proof; it does not own the runner. The orchestrator ranks and routes the request, the proof broker runs the command, and the resource broker enforces the lease. Work is eligible only when it is relevant to the PR, centrally scheduled, deduped, budgeted, leased, receipted, and likely to change the review decision.
