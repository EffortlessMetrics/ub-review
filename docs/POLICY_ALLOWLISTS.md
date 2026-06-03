# Policy allowlists

The repository policy model is deny by default and allow only by structured
receipt. TOML allowlists are used for exceptions that strict local checks cannot
reason about safely on their own.

Current allowlist families:

- `policy/no-panic-allowlist.toml` for panic-family exceptions;
- `policy/clippy-exceptions.toml` for lint suppressions;
- `policy/non-rust-allowlist.toml` for non-Rust surfaces;
- `policy/generated-allowlist.toml` for generated files;
- `policy/process-allowlist.toml` for process-spawning surfaces;
- `policy/network-allowlist.toml` for network-capable surfaces;
- `policy/unsafe-allowlist.toml` for unsafe islands;
- `policy/ripr-suppressions.toml` for static mutation-exposure waivers.

Receipts should be semantic and stable under ordinary refactors. Prefer
selectors such as path, family, container, callee, glob, surface, and owner over
line-only entries.
