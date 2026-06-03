# ripr lane doctrine

> **Doctrine:** ripr is static mutation-exposure analysis. It catches much of
> the same signal mutation testing catches, but earlier and cheaper. Runtime
> mutation testing remains the empirical backstop for what static analysis cannot
> predict. At industrialized-agent throughput, that distinction is not cosmetic:
> it is how deep verification remains economically sustainable.

`ub-review` already treats `ripr` as a sensor surface in evidence packets. That
sensor should remain a shift-left signal, not a reason to run every expensive
mutation lane on every pull request.

## Expected staging

```text
ripr/static exposure signal per PR when available
→ targeted runtime mutation for risky, ambiguous, or release-relevant surfaces
→ release receipts that state what was covered and what remained untested
```

## Claim boundary

ripr can support claims about statically exposed mutation-like risk. It does not
prove that runtime tests kill mutants, that behavior is correct, or that release
readiness has been established.
