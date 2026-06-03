# Verification Ladder

The default PR path should be boring, sharp, local, deterministic, and
Linux-first. It should answer:

> Did this PR plausibly break the changed crate, its direct dependents, or the canonical supported path?

It should not try to answer every expensive validation question on every PR.
Deep validation belongs on `main`, nightly, release, campaign, hardware, or
explicitly label-gated PR lanes.

| Layer | Default PR? | Role |
| --- | ---: | --- |
| `cargo check` | yes | type and feature wiring |
| `cargo fmt` | yes | mechanical consistency |
| `cargo clippy` | yes | lint/static policy |
| unit/oracle tests | yes | deterministic behavior proof |
| action smoke | yes | packaged action and artifact contract proof |
| `ripr` | advisory/label first | static oracle-gap signal |
| property tests | selective | bounded input confidence |
| coverage | main/label | execution surface measurement |
| mutation testing | nightly/label | runtime adequacy calibration |
| model smoke | label/manual | provider and model integration proof |
| release packaging | path/label/tag | binary packaging and publication proof |

## Default PR constraints

Default PR-worthy checks should be:

- deterministic;
- local where possible;
- cached;
- no large model by default;
- no Docker unless Docker changed;
- no macOS unless platform risk exists;
- no GPU unless GPU risk exists;
- no full mutation testing;
- no full coverage;
- no broad feature/platform matrix unless manifests or platform code changed.
