#!/usr/bin/env bash
set -euo pipefail

readonly RELEASE_TAG="v0.1.0"
readonly RELEASE_COMMIT="743ae2b5d9b9532852f702a095cf363380c932a2"
readonly RELEASE_ASSET="ub-review-x86_64-unknown-linux-gnu.tar.gz"
readonly RELEASE_ASSET_ID="481332264"
readonly RELEASE_CHECKSUM_ASSET_ID="481332265"
readonly RELEASE_SIZE="2750491"
readonly RELEASE_SHA256="87a660273e8d6f76d78b41d5bf2da1ed2928cb7987fbe114f9e1035d32b03465"
readonly RELEASE_URL="https://github.com/EffortlessMetrics/ub-review/releases/download/${RELEASE_TAG}/${RELEASE_ASSET}"
readonly RELEASE_CHECKSUM_URL="${RELEASE_URL}.sha256"
readonly MINIMUM_GLIBC="2.39"

out_arg="${1:-target/release-portability}"
mkdir -p "$out_arg"
readonly OUT_DIR="$(cd "$out_arg" && pwd -P)"

if ! command -v docker >/dev/null 2>&1; then
  echo "docker is required for the clean-container portability proof" >&2
  exit 1
fi

run_row() {
  local image="$1"
  local expectation="$2"
  local row_name="$3"
  local row_dir="$OUT_DIR/$row_name"

  rm -rf "$row_dir"
  mkdir -p "$row_dir"

  docker run --rm --interactive \
    --network bridge \
    --mount "type=bind,src=$row_dir,dst=/receipt" \
    --env "RELEASE_TAG=$RELEASE_TAG" \
    --env "RELEASE_COMMIT=$RELEASE_COMMIT" \
    --env "RELEASE_ASSET=$RELEASE_ASSET" \
    --env "RELEASE_ASSET_ID=$RELEASE_ASSET_ID" \
    --env "RELEASE_CHECKSUM_ASSET_ID=$RELEASE_CHECKSUM_ASSET_ID" \
    --env "RELEASE_SIZE=$RELEASE_SIZE" \
    --env "RELEASE_SHA256=$RELEASE_SHA256" \
    --env "RELEASE_URL=$RELEASE_URL" \
    --env "RELEASE_CHECKSUM_URL=$RELEASE_CHECKSUM_URL" \
    --env "MINIMUM_GLIBC=$MINIMUM_GLIBC" \
    --env "PROOF_IMAGE=$image" \
    "$image" bash -s -- "$expectation" <<'CONTAINER'
set -euo pipefail

expectation="$1"
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq --no-install-recommends ca-certificates curl git python3 >/receipt/apt.log

if command -v cargo >/dev/null 2>&1; then
  echo "cargo unexpectedly present: $(command -v cargo)" >&2
  exit 1
fi
if command -v rustc >/dev/null 2>&1; then
  echo "rustc unexpectedly present: $(command -v rustc)" >&2
  exit 1
fi

mkdir -p /work/download /work/install
cp /etc/os-release /receipt/os-release
uname -a > /receipt/uname.txt
uname -m > /receipt/arch.txt
getconf GNU_LIBC_VERSION > /receipt/glibc.txt
printf '%s\n' "$PROOF_IMAGE" > /receipt/container-image.txt
printf '%s\n' \
  "cargo=absent" \
  "rustc=absent" \
  "ub_review_source_checkout=absent" \
  > /receipt/prerequisites.txt

archive="/work/download/$RELEASE_ASSET"
checksum="${archive}.sha256"
curl --fail --location --retry 3 --retry-delay 2 --silent --show-error \
  --output "$archive" "$RELEASE_URL"
curl --fail --location --retry 3 --retry-delay 2 --silent --show-error \
  --output "$checksum" "$RELEASE_CHECKSUM_URL"

actual_size="$(stat -c '%s' "$archive")"
if [[ "$actual_size" != "$RELEASE_SIZE" ]]; then
  echo "release asset size mismatch: expected $RELEASE_SIZE, got $actual_size" >&2
  exit 1
fi
remote_sha="$(awk 'NF >= 1 {print $1; exit}' "$checksum")"
if [[ "$remote_sha" != "$RELEASE_SHA256" ]]; then
  echo "release checksum receipt mismatch: expected $RELEASE_SHA256, got $remote_sha" >&2
  exit 1
fi
actual_sha="$(sha256sum "$archive" | awk '{print $1}')"
if [[ "$actual_sha" != "$RELEASE_SHA256" ]]; then
  echo "release asset digest mismatch: expected $RELEASE_SHA256, got $actual_sha" >&2
  exit 1
fi
cp "$checksum" /receipt/published-checksum.txt
printf '%s  %s\n' "$remote_sha" "$archive" \
  | sha256sum --check - > /receipt/checksum-verification.txt

python3 - "$archive" /receipt/archive-layout.json <<'PY'
import json
import sys
import tarfile
from pathlib import PurePosixPath

archive, receipt = sys.argv[1:]
with tarfile.open(archive, "r:gz") as bundle:
    members = bundle.getmembers()
    described = []
    for member in members:
        raw_name = member.name
        normalized = raw_name[2:] if raw_name.startswith("./") else raw_name
        path = PurePosixPath(normalized)
        safe = (
            normalized == "ub-review"
            and not path.is_absolute()
            and ".." not in path.parts
            and member.isfile()
            and not member.issym()
            and not member.islnk()
        )
        described.append(
            {
                "name": raw_name,
                "normalized_name": normalized,
                "regular_file": member.isfile(),
                "symlink": member.issym(),
                "hardlink": member.islnk(),
                "safe_root_executable_candidate": safe,
                "size": member.size,
            }
        )
    valid = len(members) == 1 and described[0]["safe_root_executable_candidate"]
    payload = {
        "schema": "ub-review.release_archive_layout.v1",
        "member_count": len(members),
        "members": described,
        "valid": valid,
    }
    with open(receipt, "w", encoding="utf-8") as handle:
        json.dump(payload, handle, indent=2, sort_keys=True)
        handle.write("\n")
    if not valid:
        raise SystemExit(
            "release archive is not exactly one root-level regular file named ub-review"
        )
PY

tar -xzf "$archive" -C /work/install
bin="/work/install/ub-review"
if [[ ! -f "$bin" || -L "$bin" ]]; then
  echo "extracted candidate is not a regular non-symlink file" >&2
  exit 1
fi
chmod +x "$bin"

set +e
version_output="$("$bin" --version 2>&1)"
version_status=$?
set -e
printf '%s\n' "$version_output" > /receipt/version.txt
printf '%s\n' "$version_status" > /receipt/version-exit-status.txt

if [[ "$expectation" == "unsupported_glibc" ]]; then
  if [[ "$version_status" -eq 0 ]]; then
    echo "Ubuntu 22.04 unexpectedly executed the v0.1.0 binary" >&2
    exit 1
  fi
  if ! grep -Fq "GLIBC_${MINIMUM_GLIBC}" /receipt/version.txt; then
    echo "expected GLIBC_${MINIMUM_GLIBC} loader failure, got:" >&2
    cat /receipt/version.txt >&2
    exit 1
  fi

  python3 - /receipt/receipt.json <<'PY'
import json
import os
import platform
import subprocess
import sys

out = sys.argv[1]
libc = subprocess.check_output(["getconf", "GNU_LIBC_VERSION"], text=True).strip()
payload = {
    "schema": "ub-review.release_portability_receipt.v1",
    "release": {
        "tag": os.environ["RELEASE_TAG"],
        "tag_commit": os.environ["RELEASE_COMMIT"],
        "asset": os.environ["RELEASE_ASSET"],
        "asset_id": int(os.environ["RELEASE_ASSET_ID"]),
        "checksum_asset_id": int(os.environ["RELEASE_CHECKSUM_ASSET_ID"]),
        "size": int(os.environ["RELEASE_SIZE"]),
        "sha256": os.environ["RELEASE_SHA256"],
        "url": os.environ["RELEASE_URL"],
    },
    "platform": {
        "container_image": os.environ["PROOF_IMAGE"],
        "architecture": platform.machine(),
        "kernel": platform.release(),
        "libc": libc,
    },
    "environment": {
        "cargo_present": False,
        "rustc_present": False,
        "ub_review_source_checkout_present": False,
        "source_build_used": False,
    },
    "checks": {
        "download": "pass",
        "published_checksum": "pass",
        "expected_digest": "pass",
        "archive_layout": "pass",
        "binary_identity": "blocked_before_start",
        "doctor": "not_run",
        "model_off_packet": "not_run",
    },
    "outcome": "unsupported_expected",
    "reason": "glibc_too_old",
    "minimum_glibc": os.environ["MINIMUM_GLIBC"],
}
with open(out, "w", encoding="utf-8") as handle:
    json.dump(payload, handle, indent=2, sort_keys=True)
    handle.write("\n")
PY
  exit 0
fi

if [[ "$version_status" -ne 0 ]]; then
  echo "supported image could not execute release binary:" >&2
  cat /receipt/version.txt >&2
  exit 1
fi
if [[ "$version_output" != "ub-review 0.1.0" ]]; then
  echo "release identity mismatch: $version_output" >&2
  exit 1
fi
"$bin" --help > /receipt/help.txt
if ! grep -Fq "Build box-aware evidence packets" /receipt/help.txt; then
  echo "help output did not identify the UB Review product" >&2
  exit 1
fi

# Negative controls prove that no bad asset reaches execution. There is no
# Cargo, Rust compiler, UB Review source checkout, or fallback path in this
# container.
negative_dir=/work/negative
mkdir -p "$negative_dir"
cp "$archive" "$negative_dir/tampered.tar.gz"
printf 'tamper\n' >> "$negative_dir/tampered.tar.gz"
if [[ "$(sha256sum "$negative_dir/tampered.tar.gz" | awk '{print $1}')" == "$RELEASE_SHA256" ]]; then
  echo "tampered archive retained the published digest" >&2
  exit 1
fi

mkdir -p "$negative_dir/nested"
cp "$bin" "$negative_dir/nested/ub-review"
tar -czf "$negative_dir/wrong-layout.tar.gz" -C "$negative_dir" nested
if python3 - "$negative_dir/wrong-layout.tar.gz" <<'PY'
import sys
import tarfile

archive = sys.argv[1]
with tarfile.open(archive, "r:gz") as bundle:
    members = bundle.getmembers()
valid = (
    len(members) == 1
    and members[0].name.lstrip("./") == "ub-review"
    and members[0].isfile()
)
raise SystemExit(0 if valid else 1)
PY
then
  echo "nested wrong-layout archive was accepted" >&2
  exit 1
fi

mkdir -p "$negative_dir/impostor"
cp /bin/true "$negative_dir/impostor/ub-review"
tar -czf "$negative_dir/impostor.tar.gz" -C "$negative_dir/impostor" ub-review
mkdir -p "$negative_dir/impostor-extract"
tar -xzf "$negative_dir/impostor.tar.gz" -C "$negative_dir/impostor-extract"
impostor_version="$($negative_dir/impostor-extract/ub-review --version 2>&1 || true)"
if [[ "$impostor_version" == "ub-review 0.1.0" ]]; then
  echo "checksum-valid impostor passed product/version identity" >&2
  exit 1
fi

set +e
curl --fail --silent --show-error --output "$negative_dir/missing" \
  "file:///work/negative/does-not-exist" >/receipt/missing-asset.stderr 2>&1
missing_status=$?
set -e
if [[ "$missing_status" -eq 0 ]]; then
  echo "missing asset control unexpectedly downloaded" >&2
  exit 1
fi

cat > /receipt/negative-controls.json <<JSON
{
  "schema": "ub-review.release_portability_negatives.v1",
  "cargo_present": false,
  "rustc_present": false,
  "ub_review_source_checkout_present": false,
  "source_fallback_available": false,
  "missing_asset_rejected": true,
  "tampered_asset_rejected_before_extraction": true,
  "wrong_layout_rejected_before_execution": true,
  "checksum_valid_impostor_rejected_by_identity": true
}
JSON

repo=/work/fixture
mkdir -p "$repo/src"
cat > "$repo/Cargo.toml" <<'TOML'
[package]
name = "release-portability-fixture"
version = "0.1.0"
edition = "2021"
TOML
cat > "$repo/src/lib.rs" <<'RS'
pub fn checked_add(left: usize, right: usize) -> Option<usize> {
    left.checked_add(right)
}
RS

cd "$repo"
git init -q -b main
git config user.name "UB Review Portability Proof"
git config user.email "proof@invalid.example"

# Exercise the released initializer exactly and retain its generated policy.
# Its known gate failure is evidence, not the runtime portability verdict.
"$bin" init \
  --path init-generated.ub-review.toml \
  --no-guide \
  --profile gh-runner \
  --force \
  > /receipt/init.stdout 2> /receipt/init.stderr
cp init-generated.ub-review.toml /receipt/init-generated.ub-review.toml

# Use the smallest explicit valid policy to isolate runtime support from the
# released initializer's empty provider and impact values.
cat > .ub-review.toml <<'TOML'
profile = "gh-runner"

[providers]
policy = "auto"

[impact]
mode = "shadow"
TOML
cp .ub-review.toml /receipt/minimal-valid.ub-review.toml

git add Cargo.toml src/lib.rs .ub-review.toml init-generated.ub-review.toml
git commit -q -m "fixture base"
cat >> src/lib.rs <<'RS'

pub unsafe fn read_raw(value: *const usize) -> usize {
    // SAFETY: the proof fixture intentionally leaves reviewable pointer evidence.
    unsafe { *value }
}
RS
git add src/lib.rs
git commit -q -m "fixture head"
git rev-parse HEAD~1 > /receipt/base-sha.txt
git rev-parse HEAD > /receipt/head-sha.txt

"$bin" doctor \
  --root "$repo" \
  --config "$repo/init-generated.ub-review.toml" \
  --profile gh-runner \
  --base HEAD~1 \
  > /receipt/doctor-init-generated.stdout \
  2> /receipt/doctor-init-generated.stderr

"$bin" doctor \
  --root "$repo" \
  --config "$repo/.ub-review.toml" \
  --profile gh-runner \
  --base HEAD~1 \
  > /receipt/doctor.stdout 2> /receipt/doctor.stderr

init_out="$repo/packet-init-generated"
"$bin" run \
  --root "$repo" \
  --base HEAD~1 \
  --head HEAD \
  --config "$repo/init-generated.ub-review.toml" \
  --out "$init_out" \
  --profile gh-runner \
  --dry-run \
  --posting artifact-only \
  --run-pass manual \
  --model-mode off \
  --fail-on-gate false \
  --no-github-summary \
  > /receipt/run-init-generated.stdout \
  2> /receipt/run-init-generated.stderr

python3 - \
  "$init_out/review/gate_outcome.json" \
  /receipt/init-generated-gate-outcome.json <<'PY'
import json
import pathlib
import sys

source = pathlib.Path(sys.argv[1])
receipt = pathlib.Path(sys.argv[2])
gate = json.loads(source.read_text())
if gate.get("schema") != "ub-review.gate_outcome.v1":
    raise SystemExit(f"unexpected init-generated gate schema: {gate.get('schema')!r}")
if gate.get("conclusion") != "fail":
    raise SystemExit(
        f"init-generated config no longer records the expected policy failure: {gate!r}"
    )
reasons = gate.get("reasons")
if not isinstance(reasons, list):
    raise SystemExit("init-generated gate reasons are not a list")
observed = {(reason.get("kind"), reason.get("id")) for reason in reasons}
expected = {
    ("policy", "impact.mode"),
    ("policy", "providers"),
}
if observed != expected:
    raise SystemExit(
        f"unexpected init-generated policy reasons: expected {sorted(expected)!r}, "
        f"got {sorted(observed)!r}"
    )
receipt.write_text(json.dumps(gate, indent=2, sort_keys=True) + "\n")
PY

out="$repo/packet"
"$bin" run \
  --root "$repo" \
  --base HEAD~1 \
  --head HEAD \
  --config "$repo/.ub-review.toml" \
  --out "$out" \
  --profile gh-runner \
  --dry-run \
  --posting artifact-only \
  --run-pass manual \
  --model-mode off \
  --fail-on-gate false \
  --no-github-summary \
  > /receipt/run.stdout 2> /receipt/run.stderr

for required in \
  running-summary.md \
  review/review.json \
  review/terminal_state.json \
  review/gate_outcome.json
  do
  if [[ ! -s "$out/$required" ]]; then
    echo "required packet artifact missing or empty: $required" >&2
    exit 1
  fi
done

python3 - "$out" /receipt/packet-inventory.json <<'PY'
import hashlib
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
receipt = pathlib.Path(sys.argv[2])
entries = []
for path in sorted(p for p in root.rglob("*") if p.is_file()):
    relative = path.relative_to(root).as_posix()
    data = path.read_bytes()
    if path.suffix == ".json":
        json.loads(data)
    elif path.suffix == ".ndjson":
        for line_number, line in enumerate(data.decode("utf-8").splitlines(), start=1):
            if line.strip():
                try:
                    json.loads(line)
                except json.JSONDecodeError as error:
                    raise SystemExit(f"{relative}:{line_number}: {error}") from error
    entries.append(
        {
            "path": relative,
            "size": len(data),
            "sha256": hashlib.sha256(data).hexdigest(),
        }
    )

gate = json.loads((root / "review/gate_outcome.json").read_text())
terminal = json.loads((root / "review/terminal_state.json").read_text())
review = json.loads((root / "review/review.json").read_text())
if gate.get("schema") != "ub-review.gate_outcome.v1":
    raise SystemExit(f"unexpected gate schema: {gate.get('schema')!r}")
if gate.get("conclusion") != "pass":
    raise SystemExit(f"minimal model-off gate did not pass: {gate!r}")
if terminal.get("schema") != "ub-review.terminal_state.v1":
    raise SystemExit(f"unexpected terminal-state schema: {terminal.get('schema')!r}")
if terminal.get("status") != "artifact-only":
    raise SystemExit(f"unexpected terminal state: {terminal!r}")
if review.get("terminal_state") != terminal:
    raise SystemExit("review.json terminal_state does not match terminal_state.json")

payload = {
    "schema": "ub-review.release_packet_inventory.v1",
    "artifact_count": len(entries),
    "artifacts": entries,
    "gate_conclusion": gate["conclusion"],
    "terminal_status": terminal["status"],
    "all_json_parsed": True,
    "all_ndjson_parsed": True,
}
receipt.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")
PY

cp "$out/running-summary.md" /receipt/running-summary.md
cp "$out/review/gate_outcome.json" /receipt/gate_outcome.json
cp "$out/review/terminal_state.json" /receipt/terminal_state.json

python3 - /receipt/receipt.json <<'PY'
import json
import os
import platform
import subprocess
import sys

out = sys.argv[1]
libc = subprocess.check_output(["getconf", "GNU_LIBC_VERSION"], text=True).strip()
payload = {
    "schema": "ub-review.release_portability_receipt.v1",
    "release": {
        "tag": os.environ["RELEASE_TAG"],
        "tag_commit": os.environ["RELEASE_COMMIT"],
        "asset": os.environ["RELEASE_ASSET"],
        "asset_id": int(os.environ["RELEASE_ASSET_ID"]),
        "checksum_asset_id": int(os.environ["RELEASE_CHECKSUM_ASSET_ID"]),
        "size": int(os.environ["RELEASE_SIZE"]),
        "sha256": os.environ["RELEASE_SHA256"],
        "url": os.environ["RELEASE_URL"],
    },
    "platform": {
        "container_image": os.environ["PROOF_IMAGE"],
        "architecture": platform.machine(),
        "kernel": platform.release(),
        "libc": libc,
    },
    "environment": {
        "cargo_present": False,
        "rustc_present": False,
        "ub_review_source_checkout_present": False,
        "source_build_used": False,
    },
    "checks": {
        "download": "pass",
        "published_checksum": "pass",
        "expected_digest": "pass",
        "archive_layout": "pass",
        "binary_identity": "pass",
        "help": "pass",
        "init_command": "pass",
        "init_generated_config_gate": "known_policy_fail",
        "doctor_init_generated_config": "pass",
        "doctor_minimal_valid_config": "pass",
        "minimal_valid_config_gate": "pass",
        "model_off_packet": "pass",
        "packet_json": "pass",
        "negative_controls": "pass",
    },
    "known_defects": [
        {
            "id": "v0.1.0-init-empty-policy-defaults",
            "scope": "init-generated config",
            "gate_reason_ids": ["impact.mode", "providers"],
            "portability_blocking": False,
        }
    ],
    "outcome": "supported_pass",
    "minimum_glibc": os.environ["MINIMUM_GLIBC"],
}
with open(out, "w", encoding="utf-8") as handle:
    json.dump(payload, handle, indent=2, sort_keys=True)
    handle.write("\n")
PY
CONTAINER
}

run_row "ubuntu:24.04" "supported" "ubuntu-24.04"
run_row "ubuntu:22.04" "unsupported_glibc" "ubuntu-22.04"

python3 - "$OUT_DIR" <<'PY'
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
rows = []
for name in ("ubuntu-24.04", "ubuntu-22.04"):
    receipt_path = root / name / "receipt.json"
    rows.append(json.loads(receipt_path.read_text()))

outcomes = {row["platform"]["container_image"]: row["outcome"] for row in rows}
expected = {
    "ubuntu:24.04": "supported_pass",
    "ubuntu:22.04": "unsupported_expected",
}
if outcomes != expected:
    raise SystemExit(f"unexpected portability matrix: {outcomes!r}")

release_identities = {
    (
        row["release"]["tag"],
        row["release"]["tag_commit"],
        row["release"]["asset"],
        row["release"]["sha256"],
    )
    for row in rows
}
if len(release_identities) != 1:
    raise SystemExit("matrix rows did not exercise the same immutable release asset")

matrix = {
    "schema": "ub-review.release_portability_matrix.v1",
    "release": rows[0]["release"],
    "minimum_glibc": rows[0]["minimum_glibc"],
    "rows": rows,
    "decision": {
        "v0_1_0_supported_baseline": "Linux x86_64 with glibc 2.39 or newer",
        "ubuntu_24_04": "supported",
        "ubuntu_22_04": "explicitly_unsupported",
        "v0_1_0_init_generated_config": "policy_invalid_empty_defaults",
        "explicit_minimal_valid_config": "model_off_packet_pass",
        "source_fallback_used": False,
    },
}
(root / "matrix.json").write_text(json.dumps(matrix, indent=2, sort_keys=True) + "\n")
(root / "decision.md").write_text(
    "# UB Review v0.1.0 release portability\n\n"
    "- **Supported proof:** Ubuntu 24.04, Linux x86_64, glibc 2.39, no Cargo, no rustc, no UB Review source checkout.\n"
    "- **Excluded baseline:** Ubuntu 22.04, glibc 2.35; the loader fails closed on the binary's `GLIBC_2.39` requirement after checksum and archive verification.\n"
    "- **Immutable asset:** `ub-review-x86_64-unknown-linux-gnu.tar.gz` from tag `v0.1.0`, SHA-256 `87a660273e8d6f76d78b41d5bf2da1ed2928cb7987fbe114f9e1035d32b03465`.\n"
    "- **Runtime proof:** exact version/help identity, `doctor`, model-off dry-run packet, JSON/NDJSON parsing, and negative asset controls.\n"
    "- **Init defect:** the released `init` command executes, but its serialized `[providers].policy` and `[impact].mode` defaults are empty; the generated-config packet records those two policy failures. An explicit minimal valid policy produces a passing model-off packet.\n"
    "- **Boundary:** no release, tag, asset, resolver, or default was mutated by this proof.\n"
)
PY

printf 'release portability proof written to %s\n' "$OUT_DIR"
