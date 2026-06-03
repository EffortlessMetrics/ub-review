# Verification ladder

Rust is part of the verification economics model: compile-time invariants,
crate-local test selection, deterministic unit tests, feature-gated checks,
small oracle/property tests, and xtask policy checks make deep verification
cheap enough to run frequently.

| Layer | Cost | Role |
| --- | ---: | --- |
| `cargo fmt` | low | Stable code shape |
| `cargo check` / Clippy | low | Type, lint, and code-shape correctness |
| Unit / oracle tests | low | Deterministic behavior checks |
| `ripr` | low-medium | Static mutation-exposure / oracle-gap signal |
| Property tests | medium | Bounded input confidence |
| Coverage | medium-high | Execution surface |
| Mutation testing | high | Runtime adequacy confirmation |
| Crossval / hardware / model validation | high | External parity and platform proof |

Expensive lanes are not skipped; they are routed to main, nightly, release, or
explicit-label runs.
