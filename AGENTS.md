# Agent instructions

Follow the Rust-first repository style in `docs/RUST_REPO_STYLE.md`.

- Do not add shell, Python, JavaScript, or TypeScript helper scripts for
  repository automation when the behavior belongs in Rust. Prefer Rust, and add
  or extend an `xtask` control plane once policy automation is needed.
- Do not add vague exception receipts. Reasons must explain why the exception
  exists, why the Rust/default path is not suitable, who owns the surface, and
  what checks it.
- Do not use blanket lint suppressions. Prefer narrow, reasoned exceptions that
  are easy for reviewers and future checks to validate.
- Do not treat formatting, sorting, or shape-style commands as approval. They
  normalize state; they do not bless new risk.
- When adding non-Rust, generated, dependency, process, network, workflow-shell,
  executable, local-context, lint, or panic-family surfaces, update the matching
  documentation or receipt and run the matching check.
- Keep PRs small and evidence-backed. Separate production changes from support
  changes, list acceptance criteria, document commands run, and call out
  non-goals.
