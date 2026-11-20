# Agentic workflow

Agents should leave the repo more governed than they found it.

`AGENTS.md` routes delivery to
[deliver-change](../../.agents/skills/deliver-change/SKILL.md) and independent
review to [review-change](../../.agents/skills/review-change/SKILL.md). These
skills extend this procedure; they do not replace the checks or lane owner.
Manual session exercises are documented in
[AGENT_WORKFLOW_EXERCISES.md](AGENT_WORKFLOW_EXERCISES.md).

For source-only Tower review versus integrated CI evidence and the intended
late-qualification flow, use [the transition guide](TOWER_CI_TRANSITIONS.md).
It describes evidence boundaries and unknown states; it grants no activation.

Work review-fast: one coherent proof obligation per PR, with validation and
known gaps recorded before handoff or merge. Complete the objective's code,
proof, docs and contracts together; a larger coherent PR is acceptable with adequate
review and tests. Do not split those pieces merely to keep a diff small. This ladder
is an example for separately useful capabilities, not a required sequence of partial
PRs:

1. docs / doctrine;
2. consolidated TOML ledger;
3. inventory command;
4. propose command;
5. checker;
6. report;
7. advisory CI;
8. blocking allowlist;
9. strict mode later.

Use Rust and `cargo`/`xtask`-style automation for durable checks. Do not add
non-Rust files, panic-family calls, Clippy suppressions, workflows, network
surfaces, process spawning, or expensive CI lanes anonymously. Add a structured
policy receipt first or in the same PR.

The lane lead may orchestrate subagents for bounded research, implementation,
review and repair. It owns integration, final staging, validation, merge judgment
and cleanup. Multiple alternatives may be developed cheaply; only the selected
candidate intended to land gets expensive qualification. Independent objectives
may proceed concurrently, with one writer per candidate.

For an authorized implementation goal, the same useful lead normally continues
through the CI wait, complete review/Conversation inventory, actual result inspection,
separate current-repository judgment, protected merge and landing readback. An
explicit Draft-only or read-only assignment still stops at its named boundary.
A real interruption uses an exact continuation record, not a duplicate CI request.
Read deliver-change for the complete finish path. The existing #1465 admission
cutover remains separate; this guidance does not claim live triggers changed.

Run the cheapest relevant proof first. Escalate to deep proof when the changed
surface buys signal from it. `ripr` is static mutation-exposure analysis: it
shifts weak test-oracle signal left, while runtime mutation remains the slower
backstop.

## Codex Cloud setup

Choose **Settings > Codex Cloud > Environments > Edit** for this repository and
paste the entire first Bash block into **Install script**. Run it from the
repository checkout. See the [Codex Cloud environment guide][codex-cloud].

```bash
#!/usr/bin/env bash
set -euo pipefail

cd -- "$(git rev-parse --show-toplevel)"
if [[ ! -f Cargo.lock ]]; then
  echo "Missing repository Cargo.lock" >&2
  exit 1
fi

# Use the checked-in pin and its components even if an ambient override is set.
settings=$(python3 - <<'PY'
import re
import tomllib

with open("rust-toolchain.toml", "rb") as file:
    toolchain = tomllib.load(file)["toolchain"]
with open("Cargo.toml", "rb") as file:
    msrv = tomllib.load(file)["package"]["rust-version"]

pin = toolchain["channel"]
if not re.fullmatch(r"\d+\.\d+\.\d+", pin) or pin != msrv:
    raise SystemExit(f"Expected Rust pin to match Cargo.toml rust-version: {pin} != {msrv}")

print(pin)
print("--profile", toolchain["profile"], sep="\n")
for component in toolchain.get("components", []):
    print("--component", component, sep="\n")
PY
)
mapfile -t toolchain_args <<< "$settings"
pin=${toolchain_args[0]}

download() {
  curl --proto '=https' --proto-redir '=https' --tlsv1.2 -fsSL "$@"
}

# Use Node's latest release channel and fetch the immutable versioned archive.
install_node_current() (
  local arch release checksum archive unpack_dir version install_dir
  case "$(uname -sm)" in
    'Linux x86_64') arch=x64 ;;
    'Linux aarch64') arch=arm64 ;;
    *) echo "Node.js setup requires Linux x86-64 or arm64" >&2; exit 1 ;;
  esac

  release=$(
    download https://nodejs.org/dist/latest/SHASUMS256.txt |
      awk -v arch="$arch" '$2 ~ ("^node-v[0-9]+\\.[0-9]+\\.[0-9]+-linux-" arch "\\.tar\\.xz$")'
  )
  read -r checksum archive <<< "$release"
  if [[ ! $checksum =~ ^[[:xdigit:]]{64}$ || -z $archive || $release == *$'\n'* ]]; then
    echo "Could not select the latest Node.js archive and checksum" >&2
    exit 1
  fi

  version=${archive#node-}
  version=${version%%-linux-*}
  install_dir="$tools_dir/${archive%.tar.xz}"
  if [[ ! -e $install_dir && ! -L $install_dir ]]; then
    unpack_dir=$(mktemp -d "$tools_dir/.node.XXXXXXXX")
    trap 'rm -rf -- "$unpack_dir"' EXIT
    cd -- "$unpack_dir"
    download "https://nodejs.org/dist/$version/$archive" -o "$archive"
    printf '%s  %s\n' "$checksum" "$archive" | sha256sum --check --status
    tar -xJf "$archive"
    mv -nT -- "${archive%.tar.xz}" "$install_dir"
  fi
  if [[ -L $install_dir || ! -x $install_dir/bin/node ]]; then
    printf 'Incomplete Node.js install at %s. Inspect and move it aside before rerunning setup.\n' "$install_dir" >&2
    exit 1
  fi
  "$install_dir/bin/node" --version
  ln -sfnT -- "${archive%.tar.xz}" "$tools_dir/node-current"
)

tools_dir="$HOME/.local/share/ub-review/codex-cloud"

if ! command -v rustup >/dev/null 2>&1; then
  export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
fi

if ! command -v rustup >/dev/null 2>&1; then
  download https://sh.rustup.rs |
    sh -s -- -y --default-toolchain none --no-modify-path
fi
rustup toolchain install stable --profile minimal --no-self-update
rustup toolchain install "${toolchain_args[@]}" --no-self-update
mkdir -p -- "$tools_dir/bin"
install_node_current
download https://astral.sh/uv/install.sh |
  UV_UNMANAGED_INSTALL="$tools_dir/bin" sh

"$tools_dir/bin/uv" --version
rustup run stable rustc --version
rustc_version=$(rustup run "$pin" rustc --version)
if [[ $rustc_version != "rustc $pin "* ]]; then
  printf 'Expected pinned rustc %s, got %s\n' "$pin" "$rustc_version" >&2
  exit 1
fi
printf '%s\n' "$rustc_version"
rustup run "$pin" cargo --version

rustup run "$pin" cargo fetch --locked
```

Setup uses the image's Bash, Git, Python 3.11+ (for TOML parsing), and standard
Linux utilities on x86-64 or arm64. It reuses an available `rustup` or installs the
official Rust toolchain manager. The moving Rust `stable` channel is for general
tooling. The workspace's minimum supported Rust version (MSRV) is `rust-version`
in [`Cargo.toml`](../../Cargo.toml), which must match
[`rust-toolchain.toml`](../../rust-toolchain.toml). Setup installs both
toolchains and explicitly uses the pin for workspace checks and dependency fetch.

Setup also installs the latest final Node.js release from Node's official
`dist/latest` channel and the latest uv from Astral. Node.js may label that
release Current or LTS. These copies live under
`$HOME/.local/share/ub-review/codex-cloud` alongside any versions from the image.
This workspace has no Node.js or uv dependency pin. Use exact versions for any
project or release check that specifies them.

The Linux image needs the libatomic runtime for [official Node.js 25 and later
binaries][node-linux-binaries]. Debian and Ubuntu provide it as `libatomic1`.
The Install script checks the prepared Node.js binary before selecting it
or installing uv. If the runtime is missing, setup stops and the previous
Node.js selection stays in place.

Installation needs outbound access to the official Rust, Node.js, uv and Cargo
dependency sources permitted by the environment's network policy. Keep runtime
checks offline.

### Activate and verify in a task shell

The installed files persist after setup. PATH exports from the Install script
apply only to that script's shell. After a rebuild or in a new Bash task shell,
paste this block from the checkout to activate the prepared commands for that
shell and verify them offline:

```bash
tools_dir="$HOME/.local/share/ub-review/codex-cloud"
if ! command -v rustup >/dev/null 2>&1; then
  export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
fi
export PATH="$tools_dir/node-current/bin:$tools_dir/bin:$PATH"
hash -r

(
set -euo pipefail
export RUSTUP_AUTO_INSTALL=0

cd -- "$(git rev-parse --show-toplevel)"
pin=$(python3 - <<'PY'
import re
import tomllib

with open("rust-toolchain.toml", "rb") as file:
    toolchain = tomllib.load(file)["toolchain"]
with open("Cargo.toml", "rb") as file:
    msrv = tomllib.load(file)["package"]["rust-version"]

pin = toolchain["channel"]
if not re.fullmatch(r"\d+\.\d+\.\d+", pin) or pin != msrv:
    raise SystemExit(f"Expected Rust pin to match Cargo.toml rust-version: {pin} != {msrv}")

print(pin)
PY
)

if [[ $(command -v node) != "$tools_dir/node-current/bin/node" ||
      $(command -v uv) != "$tools_dir/bin/uv" ]] ||
   ! command -v rustup >/dev/null 2>&1; then
  echo "Prepared node, uv, or rustup is missing from PATH. Refresh the Install script." >&2
  exit 1
fi

installed_binary() {
  local path
  if ! path=$(command rustup which --toolchain "$1" "$2") || [[ ! -x $path ]]; then
    printf 'Missing %s for Rust toolchain %s. Refresh the Install script.\n' "$2" "$1" >&2
    return 1
  fi
  printf '%s\n' "$path"
}

stable_rustc=$(installed_binary stable rustc)
pinned_rustc=$(installed_binary "$pin" rustc)
pinned_cargo=$(installed_binary "$pin" cargo)

command node --version
command uv --version
"$stable_rustc" --version
rustc_version=$("$pinned_rustc" --version)
if [[ $rustc_version != "rustc $pin "* ]]; then
  printf 'Expected pinned rustc %s, got %s\n' "$pin" "$rustc_version" >&2
  exit 1
fi
printf '%s\n' "$rustc_version"
"$pinned_cargo" --version
RUSTC="$pinned_rustc" "$pinned_cargo" fetch --locked --offline
)
```

The check resolves the installed Rust binaries before any version probe and
checks the locked dependency cache without compiling the workspace. Refresh
the install phase after a Rust pin or lockfile change that makes the cache stale.

### Run workspace commands with the repository pin

Use the installed Cargo and rustc for the channel in `rust-toolchain.toml`.
The image's `cargo` can come first on PATH, and an explicit
`RUSTUP_TOOLCHAIN` can select another toolchain through a Rustup proxy. The
task-shell preflight verifies the pinned binaries directly and leaves the
caller's toolchain selection in place.

After activation, run this from the checkout for a pinned offline workspace
check:

```bash
(
set -euo pipefail
export RUSTUP_AUTO_INSTALL=0

cd -- "$(git rev-parse --show-toplevel)"
pin=$(python3 -c 'import tomllib; print(tomllib.load(open("rust-toolchain.toml", "rb"))["toolchain"]["channel"])')
pinned_cargo=$(command rustup which --toolchain "$pin" cargo)
pinned_rustc=$(command rustup which --toolchain "$pin" rustc)
RUSTC="$pinned_rustc" "$pinned_cargo" check --workspace --locked --offline
)
```

Substitute the workspace command you need for `check --workspace`. To use the
moving stable compiler for general tooling, select it explicitly with
`RUSTUP_AUTO_INSTALL=0 rustup run stable rustc --version`.

### Optional offline gate tools

The base setup covers source work and locked Cargo commands. Provision the
optional model-off gate tools specified in
[`.ub-review.toml`](../../.ub-review.toml) during setup when a task needs the
offline gate. Tool ordering, model-on configuration, and credentials are
separate from this bootstrap.

[codex-cloud]: https://learn.chatgpt.com/codex/environments/cloud-environments#customize-installation-and-startup
[node-linux-binaries]: https://github.com/nodejs/node/blob/v26.x/BUILDING.md#official-binary-platforms-and-toolchains
