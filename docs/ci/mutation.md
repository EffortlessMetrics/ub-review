# Mutation testing lane

Runtime mutation testing is the empirical backstop for cases where static
mutation-exposure analysis cannot provide enough confidence. It is valuable, but
it is not cheap enough to be the default answer for every pull request.

## Policy

- Use ripr/static mutation-exposure signal first when it is available.
- Run runtime mutation testing for risky seams, ambiguous static findings,
  release-relevant code, or explicitly labeled heavy verification.
- Scope mutation runs to the smallest meaningful package, module, or changed
  surface.
- Upload receipts that record the command, scope, surviving mutants, skipped
  regions, and claim boundary.

## Claim boundary

Mutation testing can show whether the selected tests killed selected generated
mutants for the selected scope. It does not prove exhaustive correctness,
security posture, fuzz robustness, or release readiness by itself.
