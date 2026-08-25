#!/usr/bin/env bash
set -euo pipefail

action_file="${1:-action.yml}"
root="$(mktemp -d)"
trap 'rm -rf "$root"' EXIT
runner="$root/source-runner.sh"

# Execute the production resolver body rather than reimplementing its branch
# logic in the fixture. The named surrounding steps bound the extracted block.
awk '
  /^    - name: Resolve ub-review runner$/ { in_step = 1; next }
  in_step && /^      run: \|$/ { capture = 1; next }
  capture && /^    - name: Install advisory sensors$/ { exit }
  capture {
    sub(/^        /, "")
    print
  }
' "$action_file" > "$runner"

if [[ ! -s "$runner" ]]; then
  echo "source runner block was not extracted" >&2
  exit 1
fi

# Record the one permitted Cargo invocation, synthesize the expected binary,
# and reproduce Cargo's stale-lock exit without network or compiler work.
make_fake_cargo() {
  local bin_dir="$1"
  mkdir -p "$bin_dir"
  cat > "$bin_dir/cargo" <<'CARGO'
#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$FAKE_CARGO_LOG"
if [ "${FAKE_CARGO_STALE:-}" = "1" ]; then
  echo "the lock file needs to be updated but --locked was passed" >&2
  exit 101
fi
target=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --target-dir)
      target="$2"
      shift 2
      ;;
    *)
      shift
      ;;
  esac
done
if [ -z "$target" ]; then
  echo "missing --target-dir" >&2
  exit 2
fi
mkdir -p "$target/release"
printf '#!/bin/sh\nexit 0\n' > "$target/release/ub-review"
chmod +x "$target/release/ub-review"
CARGO
  chmod +x "$bin_dir/cargo"
}

# Run one isolated lockfile shape through the real source resolver and assert
# both the exit contract and whether Cargo was admitted at all.
run_case() {
  local name="$1"
  local lock_shape="$2"
  local stale="$3"
  local expected_status="$4"
  local case_root="$root/$name"
  local action_root="$case_root/action"
  local runner_root="$case_root/runner"
  local fake_bin="$case_root/fake-bin"
  local cargo_log="$case_root/cargo.log"
  local output_file="$case_root/github-output"
  local target="$case_root/target-cache"
  local output="$case_root/output.txt"
  local status

  mkdir -p "$action_root" "$runner_root"
  cat > "$action_root/Cargo.toml" <<'TOML'
[package]
name = "ub-review"
version = "0.1.0"
edition = "2024"
TOML
  case "$lock_shape" in
    regular)
      printf 'version = 4\n' > "$action_root/Cargo.lock"
      ;;
    missing)
      ;;
    directory)
      mkdir "$action_root/Cargo.lock"
      ;;
    symlink)
      printf 'version = 4\n' > "$action_root/committed.lock"
      ln -s committed.lock "$action_root/Cargo.lock"
      ;;
    *)
      echo "unknown lock shape: $lock_shape" >&2
      exit 1
      ;;
  esac
  make_fake_cargo "$fake_bin"

  set +e
  PATH="$fake_bin:$PATH" \
  RUNNER_TEMP="$runner_root" \
  GITHUB_OUTPUT="$output_file" \
  GITHUB_ACTION_PATH="$action_root" \
  CARGO_TARGET_DIR="$target" \
  FAKE_CARGO_LOG="$cargo_log" \
  FAKE_CARGO_STALE="$stale" \
  UB_REVIEW_INSTALL_MODE=source \
  UB_REVIEW_BINARY_PATH= \
  UB_REVIEW_RELEASE_VERSION= \
  UB_REVIEW_RELEASE_ASSET=ub-review.tar.gz \
  UB_REVIEW_ACTION_REPOSITORY= \
  UB_REVIEW_ACTION_REF= \
  UB_REVIEW_SERVER_URL=https://fixture.invalid \
    bash "$runner" > "$output" 2>&1
  status=$?
  set -e

  if [[ "$status" != "$expected_status" ]]; then
    echo "$name: expected status $expected_status, got $status" >&2
    cat "$output" >&2
    exit 1
  fi

  case "$lock_shape" in
    regular)
      if [[ "$stale" == "1" ]]; then
        grep -Fq "the lock file needs to be updated but --locked was passed" "$output"
      else
        grep -Fq "bin=$target/release/ub-review" "$output_file"
      fi
      [[ "$(wc -l < "$cargo_log")" == "1" ]]
      grep -Fq "build --manifest-path " "$cargo_log"
      grep -Fq " --locked --release --target-dir " "$cargo_log"
      ! grep -Fq "generate-lockfile" "$cargo_log"
      ;;
    *)
      grep -Fq "source install requires a committed regular Cargo.lock" "$output"
      [[ ! -s "$cargo_log" ]]
      ;;
  esac
}

run_case regular regular 0 0
run_case missing missing 0 1
run_case directory directory 0 1
run_case symlink symlink 0 1
run_case stale regular 1 101
