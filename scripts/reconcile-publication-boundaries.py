#!/usr/bin/env python3
"""Fail-closed publication-boundary entrypoint over the retained core checker.

The core implementation remains byte-identical to the reviewed #1304 candidate.
This entrypoint adds the final completeness invariant found during exact-head
source review: a successful terminal transaction must account for every
prepared inline delivery. Retry/subset transactions remain unverifiable until
#959 can join independently validated prior-confirmation evidence.
"""
from __future__ import annotations

import importlib.util
from pathlib import Path
import sys
from typing import Any

CORE_PATH = Path(__file__).with_name("reconcile-publication-boundaries-core.py")
SPEC = importlib.util.spec_from_file_location("publication_boundaries_core", CORE_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("publication-boundary core checker could not be loaded")
core = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(core)

_original_validate_delivery_transaction = core.validate_delivery_transaction


def validate_delivery_transaction(
    packet: Any,
    transaction: Any,
    binding: dict[str, str] | None,
    prepared: dict[str, Any],
) -> bool:
    valid = _original_validate_delivery_transaction(
        packet,
        transaction,
        binding,
        prepared,
    )
    planned = transaction.get("planned") if isinstance(transaction, dict) else None
    available = prepared.get("payload", {}).get("comments")
    if isinstance(planned, list) and isinstance(available, list) and len(planned) != len(available):
        packet.unavailable(
            "delivery_transaction_incomplete",
            "review/delivery-transaction.json",
            f"{len(planned)}/{len(available)}",
        )
        valid = False
    return valid


core.validate_delivery_transaction = validate_delivery_transaction

for _name in dir(core):
    if not _name.startswith("__"):
        globals()[_name] = getattr(core, _name)
globals()["validate_delivery_transaction"] = validate_delivery_transaction


if __name__ == "__main__":
    raise SystemExit(core.main(sys.argv[1:]))
