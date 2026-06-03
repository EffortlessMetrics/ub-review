fmt:
    cargo fmt --all -- --check

lint:
    cargo clippy --workspace --all-targets --all-features --locked -- -D warnings

test:
    cargo test --workspace --all-features --locked

policy:
    cargo xtask policy-check

policy-inventory:
    cargo xtask policy-inventory

coverage:
    cargo llvm-cov --workspace --all-features --locked --lcov --output-path lcov.info

coverage-html:
    cargo llvm-cov --workspace --all-features --locked --html
