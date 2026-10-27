# Semantic configuration preflight

`doctor` and `plan` exit **1** when the selected configuration has retained
semantic policy errors. Each error is printed to stderr with the config source,
field or section, and a correction. Doctor checks this before tool availability;
an invalid config is unsuccessful even without `--require-core-tools`.

For example, `[gatee]` is an unsupported section, `[gate].target_minutes =
"oops"` needs a nonnegative integer, and an unknown `profile` needs a corrected
runtime profile name or a `[profiles.<name>]` declaration. Correct or remove
each reported field and retry. See [config examples](../configs/) for the policy
shapes and supported values. Valid sibling fields continue to apply.

```bash
ub-review doctor --config review.toml --root .
ub-review plan --config review.toml --root . --base HEAD^ --head HEAD
ub-review plan --config review.toml --root . --base HEAD^ --head HEAD --write
```

Without `--write`, plan creates no packet. With `--write`, plan keeps its
investigation artifacts, including `effective-config.json` and every policy
receipt, then exits **1**. A writer failure also exits unsuccessfully; partial
output is not a complete packet. The preceding semantic diagnostics remain
visible if writing fails. These commands execute no sensors or model providers.

Rejected values and raw parser receipt details are omitted from semantic
diagnostics. Plan's diagnostic copy of the config uses safe correction text in
each receipt. For an unknown profile, that copy records the loader's actual
`gh-runner` fallback; the policy receipt still marks the requested profile
invalid. Proof entry IDs embedded in receipt fields are redacted. Each rendered
path/field is escaped and capped at 256 characters plus a truncation marker;
every retained error is still reported. Valid config values are not redacted:
this is protection for rejected payloads, not a general secret scrubber.

Config selection retains the shared loader's precedence: explicit `--config`
wins over `UB_REVIEW_CONFIG`; either is required to be readable. With neither,
`.ub-review.toml` is optionally discovered in the working directory. Diagnostics
label these as explicit or discovered. An absent discovered config uses defaults.
Runtime profile overrides retain their existing precedence. TOML syntax and read
errors retain their existing path context and unsuccessful status.

This caller diagnosis does not change `run`: its original strip-and-receipt
behavior, policy gate reasons and enforcement rules remain in effect. Existing
run artifacts and hard syntax/read diagnostics are outside this semantic
redaction boundary; this change does not claim they scrub rejected secrets.
