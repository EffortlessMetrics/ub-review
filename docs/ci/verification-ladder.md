# Verification Ladder

Rust is part of this repository's verification economics model: compile-time invariants, crate-local tests, feature-gated checks, and typed policy tooling make meaningful PR verification cheap.

| Layer | Cost | Role |
| --- | ---: | --- |
| `cargo fmt` | low | Formatting determinism |
| `cargo check` | low | Type and feature-shape correctness |
| `cargo clippy` | low | Code-shape, panic, and suppression policy |
| Unit / oracle tests | low | Deterministic behavior checks |
| `cargo xtask` policy checks | low | Receipt and repo-surface governance |
| `ripr` | low-medium | Static mutation-exposure / oracle-gap signal |
| Property tests | medium | Bounded input confidence |
| Coverage | medium-high | Execution surface signal |
| Runtime mutation testing | high | Runtime adequacy confirmation |
| Cross-platform / hardware / model validation | high | External parity and platform proof |

Expensive lanes are not skipped. They are routed to the PRs, labels, and schedules that need them.
