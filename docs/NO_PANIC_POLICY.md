# No-panic policy

`ub-review` should be panic-averse by default. Panics make evidence tooling brittle and hide ordinary fallible conditions behind abrupt process exits.

## Covered behavior

The policy covers:

- `unwrap()`
- `expect()`
- `panic!()`
- `todo!()`
- `unimplemented!()`
- `unreachable!()`
- unchecked indexing and slicing
- string slicing by byte index
- chained patterns such as `get(...).unwrap()`

## Tests

Tests are not exempt. Assertions remain valid test oracles, but fixture setup and ordinary fallible operations should usually return `Result`.

```rust
#[test]
fn parses_fixture() -> anyhow::Result<()> {
    let input = std::fs::read_to_string("fixtures/example.txt")?;
    let parsed = parse(&input)?;
    assert_eq!(parsed.items.len(), 3);
    Ok(())
}
```

## Exceptions

Temporary exceptions belong in `policy/no-panic-allowlist.toml`. Prefer semantic selectors over line numbers so receipts survive refactors.
