# No-Panic Policy

This repository is panic-free by default in production and tests. Panic-family operations include `unwrap`, `expect`, `panic!`, `todo!`, `unimplemented!`, unchecked indexing/slicing, and equivalent assertion-driven fixture shortcuts when they hide fallibility.

Exceptions are allowed only by structured TOML receipt in `policy/no-panic-allowlist.toml`. Receipts are semantic: `path + family + selector` identifies the exception; line and column are advisory `last_seen` hints only.
