#!/usr/bin/env bash
set -euo pipefail

# Build-time installer for the standard ub-review runner image.
# GitHub-hosted fallback remains best-effort in install-gh-runner-tools.sh.

prefix="${UB_REVIEW_TOOL_DIR:-/opt/ub-review}"

install_tool() {
  local bin="$1"
  local crate="$2"
  local version="${3:-}"
  if [[ -n "$version" ]]; then
    cargo install "$crate" --version "$version" --locked --root "$prefix"
  else
    cargo install "$crate" --locked --root "$prefix"
  fi
  "$prefix/bin/$bin" --version
}

mkdir -p "$prefix/bin"

install_tool tokmd tokmd "${UB_REVIEW_TOKMD_VERSION:-1.11.1}"
install_tool ripr ripr
install_tool unsafe-review unsafe-review
install_tool ast-grep ast-grep

cat <<EOF

ub-review image tools installed.

Add this to the runner image environment:
  export PATH="$prefix/bin:\$PATH"
  export UB_REVIEW_TOOL_DIR="$prefix/bin"
  export UB_REVIEW_CACHE_DIR="/var/cache/ub-review"
  export UB_REVIEW_STANDARD_IMAGE="true"
EOF
