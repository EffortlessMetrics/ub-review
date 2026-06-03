# CI Labels

Labels authorize CI spend and deeper proof.

| Label | Meaning |
| --- | --- |
| `full-ci` | Run broad validation beyond the ordinary PR gate. |
| `ci-budget-ack` | Acknowledge elevated CI spend. |
| `ci-budget-override` | Override the hard budget ceiling. |
| `ripr` | Force static RIPR exposure analysis once the lane exists. |
| `ripr-waive` | Acknowledge a RIPR advisory finding. |
| `coverage` | Run coverage collection once available. |
| `mutation` | Run mutation testing once available. |
| `property-tests` | Run bounded property tests once available. |
| `security-audit` | Run audit/dependency/license checks once available. |
| `macos` | Run macOS lanes once available. |
| `windows` | Run Windows lanes once available. |
| `docker` | Run Docker image lanes once available. |
| `release-check` | Run release packaging dry-run checks. |
| `ub-review-model-smoke` | Run the MiniMax-backed model smoke lane. |

CI-related PRs should explain the default LEM impact, any new default lanes, any
new label/main/nightly lanes, expensive runners, cache behavior, and branch
protection impact.
