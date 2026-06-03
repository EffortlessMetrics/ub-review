# File policy

The repository is Rust-first. Non-Rust files are not forbidden, but they need explicit purpose, ownership, and review cadence.

## Preferred surfaces

- Rust source and Cargo manifests.
- TOML policy/configuration files.
- Markdown documentation.
- GitHub Actions YAML.
- Small checked-in fixtures required by tests or examples.

## Governed surfaces

New shell, Python, JavaScript, TypeScript, generated files, binary assets, or tool-specific configuration should be justified in `policy/non-rust-allowlist.toml` unless already covered by an existing pattern.

## Review rule

If a new file type adds maintenance, execution, supply-chain, or generated-artifact risk, add the allowlist receipt in the same PR as the file.
