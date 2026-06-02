# Sensor routing

Do not give every lane every artifact. Route evidence by job.

| Sensor | ub | source-route | tests | arch | opposition | security |
|---|---:|---:|---:|---:|---:|---:|
| `tokmd` | yes | yes | yes | yes | yes | yes |
| `ripr` | yes | yes | yes | no | yes | no |
| `unsafe-review` | yes | no | yes | yes | yes | yes |
| `ast-grep` | yes | yes | no | yes | yes | yes |
| `semgrep` | yes | yes | no | no | yes | yes |
| `actionlint` | no | yes | no | yes | no | yes |
| `zizmor` | no | yes | no | yes | no | yes |
| `gitleaks` | no | no | no | no | no | yes |
| dependency scanners | no | no | no | no | no | yes |

The summary reducer sees everything. Individual lanes receive the evidence that sharpens their specialty.

## Sensor issue escalation

Sensors are receipts, not local subsystems to fork inside `ub-review`. If a
real sensor issue blocks the Bun UB lane, file it upstream in the matching
`*-swarm` repo instead of hiding it behind local glue:

| Issue type | Upstream repo |
|---|---|
| `ripr` bug or weak command/output contract | `ripr-swarm` |
| `unsafe-review` bug or weak ReviewCard/witness/comment-plan contract | `unsafe-review-swarm` |
| `tokmd` bug or weak packet/manifest/context contract | `tokmd-swarm` |

Each issue should include a minimal repro, command run, expected behavior,
actual behavior, artifact excerpt, Bun UB impact, and proposed acceptance
criteria. Work around locally only when needed to keep `ub-review` usable, and
link the workaround to the upstream issue.
