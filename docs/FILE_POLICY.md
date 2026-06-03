# File Policy

Rust-first does not mean Rust-only. It means Rust and `xtask` should own implementation, automation, release checks, policy checks, fixture runners, and CI logic where practical.

Non-Rust files are allowed when their surface is explicit. Each non-Rust programming, configuration, generated, fixture, or automation file should have an allowlist receipt with owner, reason, surface, classification, and coverage.

The control plane is `policy/non-rust-allowlist.toml`. The eventual `cargo xtask check-file-policy` check should fail for unallowlisted files, missing fields, expired entries, and unused entries unless `retired = true`.
