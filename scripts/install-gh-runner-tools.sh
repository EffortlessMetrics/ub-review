#!/usr/bin/env bash
set -euo pipefail

# No-token, best-effort setup for advisory sensors on GitHub-hosted runners.
# Missing tools are safe: ub-review records them as skipped instead of failing
# the review packet.

bundle="${UB_REVIEW_TOOL_BUNDLE:-core}"

install_cargo_bin() {
  local bin="$1"
  local crate="$2"
  local version="${3:-}"
  if command -v "$bin" >/dev/null 2>&1; then
    if [[ -n "$version" ]]; then
      local current_version
      current_version="$("$bin" --version 2>/dev/null || true)"
      if [[ "$current_version" == *"$version"* ]]; then
        echo "::notice::$bin $version already available"
        return 0
      fi
      echo "::notice::$bin is available but not $version; reinstalling $crate $version"
    else
      echo "::notice::$bin already available"
      return 0
    fi
  fi
  local install_args=("$crate" "--locked")
  if [[ -n "$version" ]]; then
    install_args+=("--version" "$version" "--force")
  fi
  echo "::group::cargo install ${install_args[*]}"
  if cargo install "${install_args[@]}"; then
    echo "::notice::installed $bin"
  else
    echo "::warning::could not install $crate${version:+ $version}; $bin sensor will be skipped"
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

install_go_bin() {
  local bin="$1"
  local package="$2"
  local version="$3"
  if command -v "$bin" >/dev/null 2>&1; then
    echo "::notice::$bin already available"
    return 0
  fi
  if ! command -v go >/dev/null 2>&1; then
    echo "::warning::go unavailable; $bin sensor will be skipped"
    return 0
  fi
  local gobin="${UB_REVIEW_GO_BIN_DIR:-${GOBIN:-$HOME/go/bin}}"
  mkdir -p "$gobin"
  echo "::group::go install $package@$version"
  if GOBIN="$gobin" go install "$package@$version"; then
    echo "::notice::installed $bin"
    export PATH="$gobin:$PATH"
    if [[ -n "${GITHUB_PATH:-}" ]]; then
      echo "$gobin" >> "$GITHUB_PATH"
    fi
  else
    echo "::warning::could not install $package@$version; $bin sensor will be skipped"
  fi
  echo "::endgroup::"
}

case "$bundle" in
  none)
    echo "::notice::UB_REVIEW_TOOL_BUNDLE=none; not installing sensors"
    ;;
  core|bun-fast|full)
    tokmd_version="${UB_REVIEW_TOKMD_VERSION:-1.11.1}"
    install_cargo_bin tokmd tokmd "$tokmd_version"
    install_cargo_bin cargo-allow cargo-allow
    install_cargo_bin ripr ripr
    install_cargo_bin unsafe-review unsafe-review
    install_npm_bin ast-grep @ast-grep/cli
    install_go_bin actionlint github.com/rhysd/actionlint/cmd/actionlint "${UB_REVIEW_ACTIONLINT_VERSION:-v1.7.12}"
    ;;
  *)
    echo "::warning::unknown UB_REVIEW_TOOL_BUNDLE=$bundle; using core"
    tokmd_version="${UB_REVIEW_TOKMD_VERSION:-1.11.1}"
    install_cargo_bin tokmd tokmd "$tokmd_version"
    install_cargo_bin cargo-allow cargo-allow
    install_cargo_bin ripr ripr
    install_cargo_bin unsafe-review unsafe-review
    install_npm_bin ast-grep @ast-grep/cli
    install_go_bin actionlint github.com/rhysd/actionlint/cmd/actionlint "${UB_REVIEW_ACTIONLINT_VERSION:-v1.7.12}"
    ;;
esac

if [[ "$bundle" == "full" ]]; then
  echo "::notice::full bundle requested; optional sensors should be preinstalled or enabled explicitly"
fi

# Optional sensors are intentionally not installed by default:
# semgrep, gitleaks, osv-scanner, cargo-audit, cargo-deny, cppcheck, zizmor.
# Enable them by preinstalling in the workflow and flipping their tool policy.
