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
