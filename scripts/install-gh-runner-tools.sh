#!/usr/bin/env bash
set -euo pipefail

# No-token, best-effort setup for advisory sensors on GitHub-hosted runners.
# Missing tools are safe: ub-review records them as skipped instead of failing
# the review packet.

bundle="${UB_REVIEW_TOOL_BUNDLE:-core}"

install_cargo_bin() {
  local bin="$1"
  local crate="$2"
  if command -v "$bin" >/dev/null 2>&1; then
    echo "::notice::$bin already available"
    return 0
  fi
  echo "::group::cargo install $crate"
  if cargo install "$crate" --locked; then
    echo "::notice::installed $bin"
  else
    echo "::warning::could not install $crate; $bin sensor will be skipped"
  fi
  echo "::endgroup::"
}

install_npm_bin() {
  local bin="$1"
  local package="$2"
  if command -v "$bin" >/dev/null 2>&1; then
    echo "::notice::$bin already available"
    return 0
  fi
  if ! command -v npm >/dev/null 2>&1; then
    echo "::warning::npm unavailable; $bin sensor will be skipped"
    return 0
  fi
  echo "::group::npm install -g $package"
  npm install -g "$package" || echo "::warning::could not install $package; $bin sensor will be skipped"
  echo "::endgroup::"
}

case "$bundle" in
  none)
    echo "::notice::UB_REVIEW_TOOL_BUNDLE=none; not installing sensors"
    ;;
  core|bun-fast|full)
    install_cargo_bin tokmd tokmd
    install_cargo_bin ripr ripr
    install_cargo_bin unsafe-review unsafe-review
    install_npm_bin ast-grep @ast-grep/cli
    ;;
  *)
    echo "::warning::unknown UB_REVIEW_TOOL_BUNDLE=$bundle; using core"
    install_cargo_bin tokmd tokmd
    install_cargo_bin ripr ripr
    install_cargo_bin unsafe-review unsafe-review
    install_npm_bin ast-grep @ast-grep/cli
    ;;
esac

if [[ "$bundle" == "full" ]]; then
  echo "::notice::full bundle requested; optional sensors should be preinstalled or enabled explicitly"
fi

# Optional sensors are intentionally not installed by default:
# semgrep, gitleaks, osv-scanner, cargo-audit, cargo-deny, cppcheck.
# Enable them by preinstalling in the workflow and flipping their tool policy.
