# No-panic policy

Panic-family calls are controlled surfaces. The default posture is fallible
code paths over `unwrap`, `expect`, `panic!`, `unreachable!`, `todo!`, and
`unimplemented!`.

## Semantic receipts

Do not whitelist panic-family calls by line number. Use semantic identity:

```text
path + family + selector
```

`last_seen` line and column metadata may be recorded, but it is advisory only.
A line-based waiver says "whatever is at this line may panic." A semantic
receipt says "this specific call shape in this specific container is
temporarily allowed for this specific reason."

Receipts belong in `policy/allow.toml` and should include owner,
classification, explanation, review timing, and expiry when temporary.
