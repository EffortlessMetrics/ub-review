# Coverage

Coverage is a repository contract for generating local reports and uploading CI reports; credential material stays outside the repository.

## Default auth path

Coverage uploads use Codecov through GitHub Actions.

Preferred mode:

- GitHub Actions OIDC
- `codecov/codecov-action`
- `use_oidc: true`
- job permission: `id-token: write`

Fallback mode:

- GitHub organization Actions secret
- secret name: `CODECOV_TOKEN`
- source: Codecov Global Upload Token
- access: selected repositories only

Public repo mode:

- tokenless upload is allowed when Codecov org public repository token auth is set to `Not required`

## Rules

- Do not commit Codecov tokens.
- Do not add repo-specific Codecov secret names.
- Do not use `pull_request_target` just to expose secrets.
- Do not make coverage blocking until the baseline is stable.
- Prefer OIDC over long-lived upload tokens.

## Common failure modes

- Private repo without OIDC or `CODECOV_TOKEN`
- Missing `id-token: write` when using OIDC
- Public repo token auth changed back to `Required`
- Fork PR expecting normal Actions secrets
- Dependabot PR expecting normal Actions secrets

## Org-level setup

| Setting | Preferred state |
| --- | --- |
| Codecov public repo token authentication | `Not required`, unless the org wants stricter public upload controls |
| Codecov Global Upload Token | Generated only if needed |
| GitHub org secret | `CODECOV_TOKEN` |
| GitHub secret access | Selected repositories, not blanket access |
| Preferred GitHub Actions auth | OIDC |
| Fallback auth | Global token via org secret |
| High-risk repos | Repo-specific token or OIDC only |

GitHub's selected-repository secret scope limits where GitHub injects the token. It does not reduce what the Codecov global token can do if leaked.

## Rollout posture

1. Coverage uploads and informational Codecov checks.
2. Stabilize the baseline.
3. Require patch and project checks in branch protection.
4. Tighten thresholds only where the signal is trusted.
