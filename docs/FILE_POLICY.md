# File policy

Rust-first does not mean Rust-only. It means Rust and repo-native tooling should
own implementation, automation, release checks, policy checks, fixture runners,
and CI logic where practical.

Non-Rust files are allowed when they belong to an explicit surface such as docs,
GitHub Actions YAML, shell bootstrap scripts, Python artifact validators,
fixtures, generated metadata, or assets. Those surfaces need an allowlist receipt
with owner, reason, surface, classification, and coverage.

`policy/non-rust-allowlist.toml` is the control plane for these receipts. The
intended check should fail on unallowlisted non-Rust programming/config files,
missing metadata, expired entries, and unused entries unless explicitly retired.
