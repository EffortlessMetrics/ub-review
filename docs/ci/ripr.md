# RIPR Advisory Lane

`ripr` is reserved as a static oracle-gap signal. Its role is to ask:

> For the behavior changed in this diff, do the current tests appear to contain a discriminator that would notice if that behavior were wrong?

It is not a replacement for mutation testing, not a proof that tests are
adequate, and not a killed/survived runtime mutation report. It starts advisory
and should only become blocking after calibration.

## Files

- `ripr.toml` declares the planned advisory configuration.
- `policy/ripr-suppressions.toml` is the suppression ledger.
- `policy/ci-lanes.toml` reserves the `ripr_advisory` lane.

## Promotion sequence

```text
advisory -> findings review -> suppressions ledger -> actuals -> warnings -> selective enforcement
```
