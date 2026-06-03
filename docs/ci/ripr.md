# ripr doctrine

> **Doctrine**: ripr is static mutation-exposure analysis. It catches the same
> class of findings mutation testing catches — weak test/oracle exposure — but
> earlier and cheaper because it runs statically and can run per PR. Mutation
> testing remains the slower runtime backstop for findings static analysis
> cannot prove. ripr shifts the mutation signal left.

Do not frame `ripr` as `ripr` versus mutation testing. The intended CI shape is
`ripr` on the cheap/default path and mutation testing as targeted, nightly, or
release proof when risk warrants the runtime cost.

## Recommended lane use

```text
Default PR:
  fmt
  check
  clippy
  focused tests
  policy checks
  ripr static mutation-exposure analysis

Risk PR:
  targeted mutation for high-risk surfaces or labels

Nightly:
  broader mutation matrix

Release:
  mutation/readiness clean enough to ship
```
