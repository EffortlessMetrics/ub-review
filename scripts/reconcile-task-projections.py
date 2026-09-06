#!/usr/bin/env python3
"""Read-only, bounded cross-projection verification for TaskLedger shadow packets.

The existing artifact verifier owns revision and event replay. This command
compares its verified ledger with the existing output planes; it neither
executes work nor changes a CI, review, publication, or scheduling decision.
"""
from __future__ import annotations

import argparse
import contextlib
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import sys
import tempfile
from typing import Any

SCHEMA = "ub-review.task_projection_reconciliation.v1"
MAX_FILE_BYTES = 4 * 1024 * 1024
MAX_INPUT_BYTES = 32 * 1024 * 1024
MAX_INPUT_FILES = 256
MAX_ROWS = 16384
MAX_ISSUES = 256
MAX_REPORT_BYTES = 512 * 1024
REPORT_PATH = "review/task_projection_reconciliation.json"


def canonical(value: Any) -> bytes:
    return (json.dumps(value, sort_keys=True, separators=(",", ":"),
                       ensure_ascii=False, allow_nan=False) + "\n").encode("utf-8")


def unique_object(pairs: list[tuple[str, Any]]) -> dict:
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate JSON key")
        result[key] = value
    return result


def strict_json(data: bytes) -> Any:
    def reject_constant(value: str) -> None:
        raise ValueError("non-finite JSON number")
    return json.loads(data, object_pairs_hook=unique_object,
                      parse_constant=reject_constant)


def verifier_module() -> Any:
    path = Path(__file__).with_name("verify-bun-review-artifacts.py")
    spec = importlib.util.spec_from_file_location("ub_review_packet_verifier", path)
    if spec is None or spec.loader is None:
        raise RuntimeError("canonical packet verifier unavailable")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def label(value: Any) -> str:
    """Bound identifiers; never copy command output, prompts, or free-form reasons."""
    if not isinstance(value, str):
        return "invalid-identity"
    safe = "".join(c if c.isprintable() else "?" for c in value)
    if len(safe) <= 120:
        return safe
    return safe[:96] + "~" + hashlib.sha256(value.encode()).hexdigest()[:16]


def integer(value: Any) -> bool:
    return type(value) is int and 0 <= value <= (1 << 64) - 1


class PacketError(ValueError):
    pass


class Packet:
    def __init__(self, root: Path):
        self.root = root.resolve(strict=True)
        if not self.root.is_dir():
            raise PacketError("packet root is not a directory")
        self.sources: dict[str, dict] = {}
        self.raw: dict[str, bytes | None] = {}
        self.total = 0
        self.issues: set[tuple[str, str, str]] = set()
        self.overflow = False
        self.coverage: dict[str, str] = {}
        self.observations: set[tuple[str, str, str]] = set()

    def issue(self, code: str, path: str, identity: str = "") -> None:
        row = (code, label(path), label(identity))
        if row in self.issues:
            return
        if len(self.issues) < MAX_ISSUES:
            self.issues.add(row)
        else:
            self.overflow = True

    def observe(self, code: str, path: str, identity: str = "") -> None:
        if len(self.observations) < MAX_ISSUES:
            self.observations.add((code, label(path), label(identity)))
        else:
            self.overflow = True

    def path(self, name: str) -> Path:
        rel = PurePosixPath(name)
        if (not name or rel.is_absolute() or ".." in rel.parts or "\\" in name
                or str(rel) != name or "\x00" in name):
            raise PacketError("unsafe packet path")
        target = self.root
        for part in rel.parts:
            target = target / part
            if target.is_symlink():
                raise PacketError("symlink packet input")
        return target

    def read(self, name: str, required: bool = False) -> bytes | None:
        if name in self.raw:
            data = self.raw[name]
            if data is None and required:
                self.issue("missing_projection", name)
            return data
        if len(self.raw) >= MAX_INPUT_FILES:
            raise PacketError("input file budget exceeded")
        path = self.path(name)
        if not path.exists():
            self.raw[name] = None
            self.coverage[name] = "unavailable"
            if required:
                self.issue("missing_projection", name)
            return None
        if not path.is_file():
            raise PacketError("packet input is not a regular file")
        with path.open("rb") as stream:
            data = stream.read(MAX_FILE_BYTES + 1)
        if len(data) > MAX_FILE_BYTES or self.total + len(data) > MAX_INPUT_BYTES:
            raise PacketError("input byte budget exceeded")
        self.total += len(data)
        self.raw[name] = data
        self.sources[name] = {"path": name, "bytes": len(data),
                              "sha256": hashlib.sha256(data).hexdigest()}
        self.coverage[name] = "read"
        return data

    def load(self, name: str, shape: type, schema: str | None = None,
             required: bool = False) -> Any:
        try:
            data = self.read(name, required)
            if data is None:
                return None
            value = strict_json(data)
            if not isinstance(value, shape):
                raise PacketError("wrong JSON shape")
            if schema is not None and value.get("schema") != schema:
                raise PacketError("unsupported schema")
            return value
        except (OSError, ValueError, RecursionError):
            self.coverage[name] = "invalid"
            self.issue("invalid_projection", name)
            return None

    def rows(self, document: Any, path: str, key: str | None = None) -> list[dict]:
        if document is None:
            return []
        value = document.get(key) if key is not None else document
        if (not isinstance(value, list) or len(value) > MAX_ROWS
                or any(not isinstance(row, dict) for row in value)):
            self.issue("invalid_projection_rows", path)
            return []
        return value

    def index(self, rows: list[dict], path: str, key: str = "id") -> dict[str, dict]:
        result = {}
        for row in rows:
            identity = row.get(key)
            if not isinstance(identity, str) or not identity.strip():
                self.issue("invalid_identity", path)
            elif identity in result:
                self.issue("duplicate_identity", path, identity)
            else:
                result[identity] = row
        return result


def capture_validation(packet: Packet, path: str, operation: Any) -> Any:
    # Existing fail() uses SystemExit. Keep its untrusted diagnostic prose out
    # of the public receipt; retain only a bounded, stable diagnostic code.
    output = io.StringIO()
    try:
        with contextlib.redirect_stdout(output), contextlib.redirect_stderr(output):
            return operation()
    except (SystemExit, ValueError, OSError, KeyError, TypeError, RecursionError):
        found = re.search(r"\[([a-z_]+)\]", output.getvalue())
        packet.issue(found.group(1) if found else "ledger_integrity", path)
        return None


def reconcile(root: Path, *, legacy: bool = False, kind: str = "review") -> dict:
    packet = Packet(root)
    verifier = verifier_module()
    binding = None
    admission = packet.load("input/revision-admission.json", dict,
                            "ub-review.revision_admission.v1", required=not legacy)
    if admission is not None:
        binding = capture_validation(packet, "input/revision-admission.json",
                                     lambda: verifier.load_revision_binding(packet.root))
        if binding is not None:
            valid = capture_validation(packet, "input/revision-admission.json",
                                       lambda: (verifier._require_revision_ref(binding, "admission"), True)[1])
            if valid is None:
                binding = None
    snapshot_path = "review/task_ledger_snapshot.json"
    snapshot = packet.load(snapshot_path, dict, "ub-review.task_ledger_snapshot.v1",
                           required=not legacy)
    ledger = {}
    try:
        events = packet.read("task_ledger_events.ndjson", required=not legacy)
        if events is not None and snapshot is not None and binding is not None:
            valid = capture_validation(packet, snapshot_path, lambda: (
                verifier._verify_task_ledger_bytes(events, packet.raw[snapshot_path], binding), True)[1])
            if valid:
                ledger = packet.index(packet.rows(snapshot, snapshot_path, "tasks"), snapshot_path)
                packet.coverage["task_ledger"] = "replay_verified"
        if packet.coverage.get("task_ledger") != "replay_verified":
            packet.coverage["task_ledger"] = "legacy_unavailable" if legacy else "unverifiable"
            if (events is None) != (snapshot is None):
                packet.issue("incomplete_ledger_pair", snapshot_path)
    except (OSError, ValueError):
        packet.issue("ledger_input_budget_or_path", snapshot_path)

    def joined(row: dict, path: str, required: bool = True) -> bool:
        if binding is None:
            return False
        actual = row.get("revision")
        if actual is None and not required:
            return False
        if actual != binding:
            packet.issue("revision_mismatch", path, row.get("id", ""))
            return False
        return True

    queue_path = "work_queue.json"
    queue_doc = packet.load(queue_path, dict, "ub-review.work_queue.v1",
                            required=kind == "review" and not legacy)
    queue = packet.index(packet.rows(queue_doc, queue_path, "tasks"), queue_path)
    portfolio_path = "review/proof_portfolio.json"
    portfolio = packet.load(portfolio_path, dict, "ub-review.proof_portfolio.v1",
                            required=kind == "review" and not legacy)
    candidates = packet.index(packet.rows(portfolio, portfolio_path, "candidate_tasks"), portfolio_path)
    decisions = packet.index(packet.rows(portfolio, portfolio_path, "decisions"), portfolio_path, "task_id")
    if portfolio is not None:
        if portfolio.get("candidate_count") != len(candidates) or type(portfolio.get("candidate_count")) is not int:
            packet.issue("portfolio_count_mismatch", portfolio_path)
        selected = portfolio.get("selected_task_ids")
        if not isinstance(selected, list) or any(not isinstance(x, str) for x in selected):
            packet.issue("invalid_selection", portfolio_path)
        elif len(set(selected)) != len(selected) or any(x not in candidates for x in selected):
            packet.issue("unexplained_selection", portfolio_path)
        if set(decisions) != set(candidates):
            packet.issue("portfolio_decision_inventory", portfolio_path)

    receipt_path = "proof_receipt.json" if kind == "worker" else "review/proof_receipts.json"
    proof_doc = packet.load(receipt_path, dict if kind == "worker" else list,
                            required=not legacy)
    proofs = packet.rows([proof_doc] if kind == "worker" and proof_doc is not None else proof_doc, receipt_path)
    receipt_index = packet.index(proofs, receipt_path)
    commands: dict[str, tuple[dict, dict]] = {}
    for i, proof in enumerate(proofs):
        if proof.get("schema") != "ub-review.proof_receipt.v1":
            packet.issue("unsupported_receipt_schema", receipt_path, proof.get("id", ""))
        joined(proof, receipt_path)
        sides = set()
        if not proof.get("commands"):
            packet.issue("empty_proof_receipt", receipt_path, proof.get("id", ""))
        for j, command in enumerate(packet.rows(proof, receipt_path, "commands")):
            side = command.get("side")
            if side in sides or not isinstance(side, str):
                packet.issue("duplicate_or_invalid_side", receipt_path, proof.get("id", ""))
            if isinstance(side, str):
                sides.add(side)
            ref = f"{receipt_path}#/commands/{j}" if kind == "worker" else f"{receipt_path}#/{i}/commands/{j}"
            commands[ref] = (proof, command)

    sensor_rows = {}
    sensor_dir = packet.path("sensors")
    try:
        if sensor_dir.exists():
            with os.scandir(sensor_dir) as scan:
                names = []
                for entry in scan:
                    if len(names) >= MAX_INPUT_FILES:
                        raise PacketError("sensor inventory budget exceeded")
                    names.append(entry.name)
            for name in sorted(names):
                path = f"sensors/{name}/ub-review-sensor-status.json"
                row = packet.load(path, dict)
                if row is not None:
                    sensor_rows[path] = row
    except (OSError, ValueError):
        packet.issue("invalid_sensor_inventory", "sensors")

    credited = {}
    executed_proofs = 0
    executed_sensors = 0
    proof_ms = 0
    for task_id, task in ledger.items():
        timing = task.get("timing", {})
        executed = timing.get("process_started_at") is not None
        source = task.get("source")
        if executed:
            if source == "Sensor":
                executed_sensors += 1
            else:
                executed_proofs += 1
                proof_ms += timing.get("process_ms") or 0
        receipt = task.get("receipt") or {}
        reference = receipt.get("Created", {}).get("reference") if isinstance(receipt, dict) else None
        if reference is not None:
            if reference in credited:
                packet.issue("duplicate_receipt_credit", snapshot_path, task_id)
            credited[reference] = task
            if reference in commands:
                proof, row = commands[reference]
                expected = "proof-command-" + verifier.sanitize_artifact_name(proof["id"]) + "-" + verifier.sanitize_artifact_name(row["side"])
                if task_id != expected:
                    packet.issue("receipt_task_identity_mismatch", reference, task_id)
                mapping = {"passed": "Succeeded", "failed": "DeterministicFailure",
                           "timed_out": "TimedOut", "skipped": "Cancelled"}
                expected_disposition = mapping.get(row.get("status"))
                if row.get("status") == "skipped" and not executed:
                    expected_disposition = "SetupFailed"
                if task.get("execution_disposition") is not None and expected_disposition != task["execution_disposition"]:
                    packet.issue("conflicting_terminal", reference, task_id)
                if row.get("duration_ms") != timing.get("process_ms") and executed:
                    packet.observe("receipt_ledger_duration_domains_differ", reference, task_id)
            elif reference in sensor_rows:
                row = sensor_rows[reference]
                if task_id != "sensor-" + str(row.get("sensor")):
                    packet.issue("receipt_task_identity_mismatch", reference, task_id)
                mapping = {"ok": "Succeeded", "failed": "DeterministicFailure", "timed_out": "TimedOut"}
                if executed and mapping.get(row.get("status")) != task.get("execution_disposition"):
                    packet.issue("conflicting_terminal", reference, task_id)
            else:
                packet.issue("receipt_missing_or_unrecognized", snapshot_path, task_id)
        if executed and not reference and "CreationFailed" not in receipt:
            packet.issue("task_without_terminal_receipt", snapshot_path, task_id)
        if executed and not task.get("resources_released"):
            packet.issue("unreleased_resources", snapshot_path, task_id)
        if source == "Sensor" and executed:
            if task_id not in queue and kind == "review":
                packet.issue("executed_sensor_missing_from_queue", queue_path, task_id)
            elif queue.get(task_id, {}).get("status") == "planned":
                packet.issue("successful_sensor_left_planned" if task.get("execution_disposition") == "Succeeded"
                             else "terminal_task_left_planned", queue_path, task_id)
        for consumer in task.get("consumers", []):
            if consumer.get("requirement") == "Required" and task.get("execution_disposition") != "Succeeded":
                # Source proposals do not own execution credit. Required request
                # satisfaction is checked below through exact request_ids.
                if not task_id.startswith("proof-request-"):
                    packet.observe("required_consumer_unsatisfied", snapshot_path, task_id)

    for ref, (proof, command) in commands.items():
        ran = command.get("status") in {"passed", "failed", "timed_out"}
        if not ran:
            continue
        if ledger and ref not in credited:
            packet.issue("receipt_without_task", ref, proof.get("id", ""))
        if kind == "review":
            identity = proof.get("id")
            if identity not in queue:
                packet.issue("executed_proof_missing_from_queue", queue_path, identity)
            if identity not in candidates:
                packet.issue("executed_proof_missing_from_portfolio", portfolio_path, identity)
            if identity not in queue and identity not in candidates and "impact-planner" in proof.get("requested_by", []):
                packet.issue("executed_impact_receipt_missing_from_queue_and_portfolio", receipt_path, identity)

    for task_id, task in ledger.items():
        existing = task.get("existing_receipt")
        if existing is not None:
            reference = existing.get("reference") if isinstance(existing, dict) else existing
            if reference not in commands and reference not in sensor_rows:
                packet.issue("existing_receipt_missing", snapshot_path, task_id)
            elif reference not in credited:
                packet.issue("existing_receipt_without_executed_owner", snapshot_path, task_id)

    if ledger:
        for identity, row in queue.items():
            if row.get("kind") == "sensor" and identity not in ledger:
                packet.issue("queue_sensor_without_ledger", queue_path, identity)
            if identity in candidates:
                decision = decisions.get(identity, {})
                if str(decision.get("status", "")).startswith(("deferred", "skipped", "refused")) and row.get("status") == "planned":
                    packet.issue("terminal_portfolio_task_left_planned", queue_path, identity)
        for identity, candidate in candidates.items():
            if identity not in queue:
                packet.issue("candidate_missing_from_queue", portfolio_path, identity)
            command_tasks = [task for ref, task in credited.items() if ref in commands and commands[ref][0].get("id") == identity]
            for task in command_tasks:
                required = any(c.get("requirement") == "Required" for c in task.get("consumers", []))
                if type(candidate.get("required")) is not bool or candidate["required"] != required:
                    packet.issue("candidate_requiredness_mismatch", portfolio_path, identity)

    for path, row in sensor_rows.items():
        task_id = "sensor-" + str(row.get("sensor"))
        if row.get("status") in {"ok", "failed", "timed_out"}:
            if ledger and path not in credited:
                packet.issue("receipt_without_task", path, task_id)
            if row.get("status") == "ok" and queue.get(task_id, {}).get("status") == "planned":
                packet.issue("successful_sensor_left_planned", queue_path, task_id)

    lease_path = "resource_lease.json" if kind == "worker" else "review/resource_leases.json"
    lease_doc = packet.load(lease_path, dict if kind == "worker" else list, required=not legacy)
    leases = packet.rows([lease_doc] if kind == "worker" and lease_doc is not None else lease_doc, lease_path)
    lease_index = packet.index(leases, lease_path)
    for lease in leases:
        joined(lease, lease_path)
        if lease.get("schema") != "ub-review.resource_lease.v1":
            packet.issue("unsupported_lease_schema", lease_path, lease.get("id", ""))
        if lease.get("status") == "granted" and lease.get("consumer") not in receipt_index:
            packet.issue("granted_lease_without_receipt", lease_path, lease.get("id", ""))
    for proof in proofs:
        if any(c.get("status") in {"passed", "failed", "timed_out"} for c in proof.get("commands", [])):
            if not legacy and not any(l.get("consumer") == proof.get("id") and l.get("status") == "granted" for l in leases):
                packet.issue("executed_proof_without_lease", lease_path, proof.get("id", ""))

    gate_path = "review/gate_outcome.json"
    gate = packet.load(gate_path, dict, "ub-review.gate_outcome.v1", required=kind == "review" and not legacy)
    if gate is not None:
        joined(gate, gate_path)
        required = gate.get("required_proof", {})
        counts = [required.get(k) for k in ("matched", "passed", "failed", "skipped")]
        if not all(integer(n) for n in counts) or counts[0] != sum(counts[1:]):
            packet.issue("required_count_mismatch", gate_path)
        elif counts[0] > counts[1]:
            packet.observe("required_proof_unproven", gate_path)
            if gate.get("conclusion") == "pass":
                packet.issue("legacy_pass_with_required_proof_unproven", gate_path)
        if gate.get("conclusion") == "pass" and gate.get("gate_result") == "not_proven":
            packet.issue("legacy_pass_with_truthful_not_proven", gate_path)

    request_path = "review/proof_requests.json"
    requests = packet.rows(packet.load(request_path, list, required=kind == "review" and not legacy), request_path)
    packet.index(requests, request_path)
    for request in requests:
        identity = request.get("id")
        if request.get("required") is True:
            satisfying = [(ref, proof) for ref, (proof, command) in commands.items()
                          if identity in proof.get("request_ids", []) and command.get("status") == "passed"
                          and (binding is None or proof.get("revision") == binding)
                          and (not ledger or ref in credited)]
            if not satisfying:
                packet.observe("required_request_without_satisfying_receipt", request_path, identity)
        if ledger and isinstance(identity, str):
            task_id = "proof-request-" + verifier.sanitize_artifact_name(identity)
            if task_id not in ledger:
                packet.issue("source_request_missing_from_ledger", request_path, identity)

    if portfolio is not None:
        if portfolio.get("budget_seconds") == 0 and any(c.get("required") is True for c in candidates.values()) and not portfolio.get("selected_task_ids"):
            packet.issue("required_proof_starved_at_zero_remaining_budget", portfolio_path)
        runtime = portfolio.get("runtime", {})
        if (portfolio.get("budget_seconds") == 0 and integer(runtime.get("deadline_remaining_seconds"))
                and runtime["deadline_remaining_seconds"] > 0
                and any(integer(c.get("duration_ms")) and integer(c.get("timeout_sec"))
                        and c["duration_ms"] < c["timeout_sec"] * 1000 for _, c in commands.values())):
            packet.issue("timeout_ceilings_exhausted_budget_before_actual_deadline", portfolio_path)
    if binding is None:
        for path, rows in [(portfolio_path, [portfolio] if portfolio else []), (receipt_path, proofs)]:
            if any(not re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", str(row.get("head", ""))) for row in rows):
                packet.issue("symbolic_head_persisted_as_authority", path)

    # Compare existing reservation quantities without inventing a scheduler.
    for ref, task in credited.items():
        reservations = {r["class"]: r["units"] for r in task.get("reservations", [])}
        if ref in commands:
            proof, command = commands[ref]
            matches = [l for l in leases if l.get("consumer") == proof.get("id") and l.get("status") == "granted"]
            if len(matches) > 1:
                packet.issue("ambiguous_command_lease", lease_path, proof.get("id", ""))
            if len(matches) == 1 and not (kind == "worker" and command.get("side") != "head"):
                lease = matches[0]
                expected = {cls: lease.get(key, 0) for cls, key in
                            [("Cpu", "cpu"), ("Memory", "memory_mb"), ("Disk", "disk_mb")]}
                expected = {cls: value for cls, value in expected.items() if value > 0}
                expected["Build" if proof.get("kind") == "focused-build" else "Test"] = 1
                if command.get("side") == "base-plus-tests" and lease.get("worktree") is not None:
                    expected["Worktree"] = 1
                if lease.get("network") is True:
                    expected["Network"] = 1
                if reservations != expected:
                    packet.issue("lease_reservation_mismatch", ref, task["id"])
        elif ref in sensor_rows and task["id"] in queue:
            lease = queue[task["id"]].get("lease", {})
            expected = {"Cpu": lease.get("cpu", 0)}
            if lease.get("disk_mb", 0) > 0:
                expected["Disk"] = lease["disk_mb"]
            if reservations != expected and task.get("timing", {}).get("admitted_at") is not None:
                packet.issue("sensor_reservation_mismatch", queue_path, task["id"])

    # Required proof counts must trace to exact source requests and complete
    # receipt outcomes, not command-text similarity or one successful side.
    if gate is not None and not legacy and kind == "review":
        required_requests = [r for r in requests if r.get("required") is True
                             and r.get("lane") == "intelligent-ci-policy"]
        classified = {"matched": len(required_requests), "passed": 0, "failed": 0, "skipped": 0}
        if gate.get("required_proof", {}).get("matched") != len(required_requests):
            packet.issue("required_request_count_mismatch", gate_path)
        for request in required_requests:
            matching = [p for p in proofs if request.get("id") in p.get("request_ids", [])]
            if len(matching) > 1:
                packet.issue("ambiguous_required_receipt", receipt_path, request.get("id", ""))
            receipt_result = matching[0].get("result") if matching else None
            bucket = ("passed" if receipt_result in {"head_passed", "discriminating"} else
                      "failed" if receipt_result in {"head_failed", "timed_out"} else "skipped")
            classified[bucket] += 1
            successful = [p for p in matching if p.get("revision") == binding
                          and p.get("result") in {"head_passed", "discriminating"}
                          and p.get("commands")
                          and all(ref in credited for ref, (owner, _) in commands.items() if owner is p)]
            if gate.get("conclusion") == "pass" and not successful:
                packet.issue("pass_without_required_receipt", gate_path, request.get("id", ""))

        if gate.get("required_proof") != classified:
            packet.issue("required_receipt_count_mismatch", gate_path)

    if kind == "worker":
        packet.coverage["worker_preflight_lease"] = "not_separately_published"

    fill_path = "review/fill-ledger.json"
    fill = packet.load(fill_path, dict, "ub-review.fill_ledger.v1")
    if fill is not None:
        entries = packet.rows(fill, fill_path, "entries")
        indexed = {}
        for entry in entries:
            key = (entry.get("kind"), entry.get("check_id"))
            if key in indexed:
                packet.issue("duplicate_fill_entry", fill_path, entry.get("check_id", ""))
            indexed[key] = entry
        for task_id, task in ledger.items():
            if task.get("source") == "Sensor" and task.get("timing", {}).get("process_started_at") is not None:
                entry = indexed.get(("sensor", task_id.removeprefix("sensor-")))
                if entry is None:
                    packet.issue("executed_sensor_missing_from_fill", fill_path, task_id)
                elif entry.get("selected") is not True:
                    packet.issue("fill_execution_selection_mismatch", fill_path, task_id)
        packet.coverage["fill_proof_identity"] = "not_equivalent_to_command_task_identity"

    scheduler_path = "review/scheduler.json"
    scheduler = packet.load(scheduler_path, dict, "ub-review.scheduler.v1")
    if scheduler is not None:
        # Phase time and command time have different origins and may overlap.
        # Compare only the duplicate projection when both fields are exposed.
        packet.coverage["scheduler_task_counts"] = "not_exposed_by_v1"

    routes_path = "review/receipt_routes.json"
    routes_doc = packet.load(routes_path, dict, "ub-review.receipt_routes.v1")
    if routes_doc is not None:
        joined(routes_doc, routes_path)
        routes = packet.rows(routes_doc, routes_path, "routes")
        packet.index(routes, routes_path)
        for route in routes:
            proof = receipt_index.get(route.get("receipt_id"))
            if proof is None or route.get("result") != proof.get("result"):
                packet.issue("route_receipt_mismatch", routes_path, route.get("id", ""))
            for lease_id in route.get("lease_ids", []):
                lease = lease_index.get(lease_id)
                if lease is None or lease.get("consumer") != route.get("receipt_id"):
                    packet.issue("route_lease_mismatch", routes_path, route.get("id", ""))

    calibration_path = "review/calibration.json"
    calibration = packet.load(calibration_path, dict, "ub-review.calibration.v0")
    if calibration is not None and ledger:
        reported = calibration.get("counts", {}).get("proof_requests_executed")
        # This v0 field counts ALL proof receipts, including skipped receipts,
        # not executed requests or physical command sides.
        count = len(proofs)
        if reported != count or not integer(reported):
            packet.issue("calibration_proof_count_mismatch", calibration_path)

    metrics_path = "review/metrics.json"
    metrics = packet.load(metrics_path, dict)
    if metrics is not None and ledger:
        measured = metrics.get("run", {}).get("proof_command_duration_ms_sum")
        receipt_ms = sum(c.get("duration_ms", 0) for _, c in commands.values() if integer(c.get("duration_ms")))
        if measured is not None and (not integer(measured) or measured != receipt_ms):
            packet.issue("proof_duration_projection_mismatch", metrics_path)
    if scheduler is not None and metrics is not None:
        for key in ("elapsed_wall_ms", "scheduler_roles", "streams", "loops", "phases"):
            if key in scheduler and key in metrics.get("run", {}) and scheduler[key] != metrics["run"][key]:
                packet.issue("scheduler_metric_mismatch", scheduler_path, key)
    cost_path = "review/ub-review-cost.json"
    cost = packet.load(cost_path, dict, "ub-review.cost_receipt.v1")
    if cost is not None:
        joined(cost, cost_path)
        # This schema contains no task count or whole-workflow measurement.
        # Absence is coverage, not zero cost or inferred physical execution.
        packet.coverage["cost_task_counts"] = "not_exposed_by_v1"
        packet.coverage["whole_workflow_cost"] = "not_measured_by_v1"

    groups = {}
    for ref, (proof, command) in commands.items():
        key = canonical({"revision": proof.get("revision"), "kind": proof.get("kind"),
                         "mode": proof.get("test_patch_mode"), "side": command.get("side"),
                         "command": command.get("command"), "env": command.get("env")})
        groups.setdefault(hashlib.sha256(key).hexdigest(), []).append(ref)
    similar = [{"candidate_fingerprint": key, "receipt_references": sorted(refs),
                "equivalence": "unproven"} for key, refs in sorted(groups.items()) if len(refs) > 1]
    packet.coverage["canonical_execution_equivalence"] = "not_available_before_860"
    packet.coverage["model_execution"] = "outside_sensor_proof_worker_ledger"
    complete = binding is not None and packet.coverage.get("task_ledger") == "replay_verified"
    status = "contradictory" if packet.issues or packet.overflow else ("coherent" if complete else "unverifiable")
    report = {"schema": SCHEMA, "packet_kind": kind, "mode": "legacy" if legacy else "current",
              "authority": "shadow-only", "status": status, "revision": binding,
              "issue_count_retained": len(packet.issues), "issues_truncated": packet.overflow,
              "issues": [{"code": c, "artifact": p, "identity": i} for c, p, i in sorted(packet.issues)],
              "observations": [{"code": c, "artifact": p, "identity": i} for c, p, i in sorted(packet.observations)],
              "counts": {"ledger_tasks": len(ledger) if complete else None,
                         "executed_sensor_tasks": executed_sensors if complete else None,
                         "executed_proof_command_tasks": executed_proofs if complete else None,
                         "proof_receipts": len(proofs), "queue_tasks": len(queue),
                         "portfolio_candidates": len(candidates)},
              "similar_command_candidates": similar[:MAX_ISSUES],
              "similar_groups_truncated": len(similar) > MAX_ISSUES,
              "coverage": dict(sorted(packet.coverage.items())),
              "source_artifacts": [packet.sources[k] for k in sorted(packet.sources)]}
    if len(canonical(report)) > MAX_REPORT_BYTES:
        raise PacketError("reconciliation report exceeds hard byte limit")
    return report


def publish_report(root: Path, report: dict) -> None:
    packet = Packet(root)
    destination = packet.path(REPORT_PATH)
    destination.parent.mkdir(parents=True, exist_ok=True)
    data = canonical(report)
    if len(data) > MAX_REPORT_BYTES:
        raise PacketError("report byte budget exceeded")
    fd, temporary = tempfile.mkstemp(prefix=".task-projections-", dir=destination.parent)
    try:
        with os.fdopen(fd, "wb") as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, destination)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("packet", type=Path)
    parser.add_argument("--kind", choices=("review", "worker"), default="review")
    parser.add_argument("--legacy", action="store_true", help="inspect retained legacy contradictions; never grants current authority")
    parser.add_argument("--write-report", action="store_true", help=f"atomically write only {REPORT_PATH}")
    args = parser.parse_args(argv)
    try:
        if args.write_report:
            old_report = Packet(args.packet).path(REPORT_PATH)
            if old_report.exists():
                old_report.unlink()
        report = reconcile(args.packet, legacy=args.legacy, kind=args.kind)
        if args.write_report:
            publish_report(args.packet, report)
        sys.stdout.buffer.write(canonical(report))
        return 0 if report["status"] == "coherent" else 1
    except (OSError, ValueError, KeyError, TypeError, RuntimeError, RecursionError):
        print("task-projection verification failed: input, output, or budget unavailable", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
