# Sensor child-process environment

`ub-review` runs advisory sensors as child processes with a deny-by-default
environment. The child starts after the inherited environment is cleared and
receives only execution metadata needed to locate tools and temporary
directories: `PATH`, platform temp variables, Windows process-launch
variables, and locale values. Home directories and Cargo/Rustup cache roots are
deliberately excluded because they may contain credentials, configuration, or
source replacements.

The regression fixture launches a dedicated test-helper process with sentinel
credentials in its real parent environment, then runs a fake sensor through the
production spawn path. This keeps the parent-environment proof isolated from
the rest of the test suite without mutating the test runner's global process
environment.

Provider credentials, GitHub tokens, OIDC variables, and unrelated ambient
runner secrets are not part of the sensor contract. This quarantine prevents
those values from reaching a sensor even when the action process needs them for
model calls or later GitHub delivery.

This is only a child-process secret-inheritance guarantee. It does not provide
hostile-head safety, trusted-base execution, repository configuration/plugin
isolation, immutable tool installation, or evidence/posting separation. Those
requirements remain tracked by issue #876.
