#!/usr/bin/env bash
set -euo pipefail

# A real diff. `--base HEAD --head HEAD` produces zero changed files, so no
# lane or sensor ever materializes and the smoke run passes without exercising
# anything — including the `init` output it just wrote.
BASE="${UB_REVIEW_SMOKE_BASE:-HEAD~1}"
OUT=target/ub-review-smoke
CONFIG=target/ub-review-smoke.toml

cargo run --locked -- doctor --profile gh-runner
cargo run --locked -- init --profile gh-runner --force --path "$CONFIG"
cargo run --locked -- plan --config "$CONFIG" --profile gh-runner --base "$BASE" --head HEAD --write --out "$OUT"
cargo run --locked -- run --config "$CONFIG" --profile gh-runner --base "$BASE" --head HEAD --dry-run --out "$OUT"

# The exit code of `run` does not carry the gate verdict, so a smoke run that
# only checks `set -e` cannot tell a clean review from a config the tool just
# rejected. Assert the verdict the consumer workflow actually branches on.
python3 - "$OUT/review/gate_outcome.json" <<'PY'
import json
import sys

path = sys.argv[1]
with open(path, encoding="utf-8") as handle:
    outcome = json.load(handle)

conclusion = outcome.get("conclusion")
if conclusion != "pass":
    reasons = outcome.get("reasons", [])
    print(f"smoke gate conclusion={conclusion!r}, expected 'pass'", file=sys.stderr)
    for reason in reasons:
        print(f"  [{reason.get('kind')}] {reason.get('id')}: {reason.get('detail')}", file=sys.stderr)
    sys.exit(1)

print(f"smoke gate conclusion=pass ({path})")
PY
