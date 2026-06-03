# ripr

`ripr` is static mutation-exposure analysis. It catches the same class of
findings mutation testing catches - weak test/oracle exposure - but earlier
and cheaper because it is static and PR-time.

Mutation testing remains the slower runtime backstop for findings static
analysis cannot predict. `ripr` shifts mutation signal left.

Treat `ripr` as an economical source of mutation-style signal, not as a second
parallel proof obligation that every PR must duplicate with full runtime
mutation. Use runtime mutation on main, nightly, release, or explicitly labeled
high-risk PRs where it buys signal.
