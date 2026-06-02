#!/usr/bin/env bash
set -euo pipefail

ub-review init --profile cx23 --force
ub-review doctor --profile cx23
ub-review plan --profile cx23 --base origin/main --head HEAD --write
ub-review run --profile cx23 --base origin/main --head HEAD
