# Release state and preparation notes

As checked on 2026-09-08, GitHub has published
[`v0.1.0`](https://github.com/EffortlessMetrics/ub-review/releases/tag/v0.1.0)
(2026-07-18) and
[`v0.1.1`](https://github.com/EffortlessMetrics/ub-review/releases/tag/v0.1.1)
(2026-08-22), each with a Linux x64 archive and checksum sibling. The earlier
`v0` and `v0.1` refs are historical tags without release archives.

The current source package version is `0.1.2`; `v0.1.2` has no published
release or remote tag in that snapshot. Source changes, including the
candidate-suggestion fingerprint parity fix (#922) and suppression-ledger
validator (#921), do not prove a release was cut. Refresh tag and release
metadata before selecting the next unused version.

The `v0.1.0` tag-push run failed and its assets were completed manually;
the `v0.1.1` tag-push run succeeded. The
[runbook](docs/RELEASE_RUNBOOK.md#current-state-and-ownership) retains those
historical run links and their limits.

Publication history does not complete the broader product, stable-tool,
provider, portability, or release-only-install acceptance in
[#805](https://github.com/EffortlessMetrics/ub-review/issues/805),
[#816](https://github.com/EffortlessMetrics/ub-review/issues/816), and
[#817](https://github.com/EffortlessMetrics/ub-review/issues/817). The current
readiness frontier is recorded in [PRODUCT_STATE](docs/PRODUCT_STATE.md).

Source surface overview:

- root `action.yml` composite action
- Rust 2024 / Rust 1.95 CLI
- `bun-ub` preset
- `gh-runner`, `cx23`, `cx33`, `cx43`, `auto`, and `custom` profiles
- best-effort `tokmd`, `cargo-allow`, `ripr`, `unsafe-review`, `ast-grep`, and
  `actionlint` sensor setup
- direct MiniMax M3 review lanes with GLM skipped for v0
- optional OpenCode Go direct provider canary lane
- grouped Pull Request Review posting with bounded inline comments
- full packet artifacts, including `review/post-result.json` or `review/post-error.json`
- standard-image doctor fails missing core tools and stale `tokmd` versions
- Bun consumer workflow example using
  `EffortlessMetrics/ub-review@804d198b5a15a0df94bb4f43750dba71165916cd`

The [release runbook](docs/RELEASE_RUNBOOK.md) owns the next-cut procedure:
select an unoccupied tag and immutable candidate SHA, retain exact proof and
asset identities, obtain explicit authorization, then verify the published
result. Documentation maintenance and a passing packaging dry run authorize
no tag, release, or other external publication.

Advancing the Bun consumer SHA requires its own verifier and consumer-run
evidence. It is a separate adoption change; a release cut does not move the
known-good Bun pin above.
