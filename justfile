fmt:
    cargo fmt --all -- --check

lint:
    cargo clippy --workspace --all-targets --all-features --locked -- -D warnings

test:
    cargo test --workspace --all-features --locked

coverage:
    cargo llvm-cov --workspace --all-features --locked --lcov --output-path lcov.info

coverage-html:
    cargo llvm-cov --workspace --all-features --locked --html
