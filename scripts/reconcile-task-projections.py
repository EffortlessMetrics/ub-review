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


def integer(value: Any, bits: int = 64) -> bool:
    return type(value) is int and 0 <= value <= (1 << bits) - 1


def proof_result_consistent(proof: dict, kind: str) -> bool:
    """Check aggregate claims against the command sides that produced them."""
    commands = proof.get("commands")
    if not isinstance(commands, list) or not commands or any(not isinstance(c, dict) for c in commands):
        return False
    sides = {c.get("side"): c.get("status") for c in commands if isinstance(c.get("side"), str)}
    if len(sides) != len(commands) or "head" not in sides:
        return False
    head, result = sides["head"], proof.get("result")
    if (not isinstance(result, str) or not isinstance(proof.get("kind"), str)
            or any(not isinstance(status, str) or status not in {"passed", "failed", "timed_out", "skipped"}
                   for status in sides.values())):
        return False
    if kind == "worker":
        if (proof.get("test_patch_mode") != "head-only"
                or set(sides) != {"head", "nightly-preflight"}):
            return False
        if result == "skipped_unresolved":
            return head == "skipped"
        expected = ("sanitizer_ub_detected"
                    if head == "failed" and proof.get("kind") == "sanitizer-witness"
                    else head)
        return result == expected
    if proof.get("kind") not in {"focused-head", "focused-build", "focused-red-green"}:
        return False
    if head != "passed":
        expected = {"failed": {"head_failed"}, "timed_out": {"timed_out"},
                    "skipped": {"skipped_profile", "skipped_budget"}}.get(head, set())
        return set(sides) == {"head"} and result in expected
    if proof.get("kind") in {"focused-head", "focused-build"}:
        return proof.get("test_patch_mode") == "head-only" and set(sides) == {"head"} and result == "head_passed"
    if proof.get("test_patch_mode") != "base-plus-tests" or set(sides) != {"head", "base-plus-tests"}:
        return False
    expected = {"failed": {"discriminating"}, "passed": {"non_discriminating"},
                "timed_out": {"timed_out"}, "skipped": {"skipped_profile", "base_patch_failed"}}
    return result in expected.get(sides["base-plus-tests"], set())


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
        self.observations_overflow = False
        self.input_unavailable = False
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
        row = (code, label(path), label(identity))
        if row in self.observations:
            return
        if len(self.observations) < MAX_ISSUES:
            self.observations.add(row)
        else:
            self.observations_overflow = True

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
        self.raw[name] = None
        path = self.path(name)
        if not path.exists():
            self.raw[name] = None
            self.coverage[name] = "unavailable"
            if required:
                self.issue("missing_projection", name)
            return None
        if not path.is_file():
            raise PacketError("packet input is not a regular file")
        remaining = MAX_INPUT_BYTES - self.total
        if remaining <= 0:
            raise PacketError("input byte budget exceeded")
        with path.open("rb") as stream:
            data = stream.read(min(MAX_FILE_BYTES, remaining) + 1)
        self.total += len(data)
        if len(data) > MAX_FILE_BYTES or self.total > MAX_INPUT_BYTES:
            raise PacketError("input byte budget exceeded")
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
            self.input_unavailable = True
            self.issue("invalid_projection", name)
            return None

    def rows(self, document: Any, path: str, key: str | None = None) -> list[dict]:
        if document is None:
            return []
        if key is not None and not isinstance(document, dict):
            self.input_unavailable = True
            self.issue("invalid_projection_rows", path, key)
            return []
        value = document.get(key) if key is not None else document
        if (not isinstance(value, list) or len(value) > MAX_ROWS
                or any(not isinstance(row, dict) for row in value)):
            self.input_unavailable = True
            self.issue("invalid_projection_rows", path, key or "rows")
            return []
        return value

    def strings(self, document: Any, path: str, key: str) -> list[str]:
        if not isinstance(document, dict):
            self.input_unavailable = True
            self.issue("invalid_projection_rows", path, key)
            return []
        value = document.get(key)
        if (not isinstance(value, list) or len(value) > MAX_ROWS
                or any(not isinstance(row, str) for row in value)):
            self.input_unavailable = True
            self.issue("invalid_projection_rows", path, key)
            return []
        return value

    def mapping(self, document: Any, path: str, key: str,
                identity: str = "") -> dict:
        if not isinstance(document, dict):
            self.input_unavailable = True
            self.issue("invalid_projection_object", path, identity or key)
            return {}
        value = document.get(key)
        if value is None:
            return {}
        if not isinstance(value, dict):
            self.input_unavailable = True
            self.issue("invalid_projection_object", path, identity or key)
            return {}
        return value

    def lease_quantities(self, lease: dict, path: str,
                         identity: str = "") -> dict[str, int] | None:
        quantities = {}
        for key in ("cpu", "memory_mb", "disk_mb", "timeout_sec"):
            value = lease.get(key)
            # ResourceLease and ProofTaskLease use u32 CPU and u64 quantities.
            if not integer(value) or (key == "cpu" and value > (1 << 32) - 1):
                self.input_unavailable = True
                self.issue("invalid_lease_quantity", path, f"{label(identity)}:{key}")
            else:
                quantities[key] = value
        return quantities if len(quantities) == 4 else None

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
    except (SystemExit, ValueError, OSError, KeyError, TypeError, AttributeError, RecursionError):
        found = re.search(r"\[([a-z_]+)\]", output.getvalue())
        packet.issue(found.group(1) if found else "ledger_integrity", path)
        return None


def worker_revision_binding(packet: Packet, verifier: Any,
                            proof: Any, lease: Any) -> dict | None:
    """Join worker output to its published revision evidence without inventing admission."""
    if not isinstance(proof, dict) or not isinstance(lease, dict):
        return None
    proof_revision = proof.get("revision")
    lease_revision = lease.get("revision")
    proof_valid = capture_validation(
        packet,
        "proof_receipt.json",
        lambda: (verifier._require_revision_ref(proof_revision, "worker proof"), True)[1],
    )
    lease_valid = capture_validation(
        packet,
        "resource_lease.json",
        lambda: (verifier._require_revision_ref(lease_revision, "worker lease"), True)[1],
    )
    if proof_valid is None or lease_valid is None:
        return None
    if proof_revision != lease_revision:
        packet.issue("worker_revision_mismatch", "resource_lease.json")
        return None
    reviewed_commit = proof_revision.get("reviewed_commit")
    if (not isinstance(reviewed_commit, str)
            or re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", reviewed_commit) is None
            or proof.get("head") != reviewed_commit):
        packet.issue("worker_revision_head_mismatch", "proof_receipt.json",
                     proof.get("id", ""))
        return None
    proof_id = proof.get("id")
    if not isinstance(proof_id, str) or not proof_id.strip() or lease.get("consumer") != proof_id:
        packet.issue("worker_revision_lease_identity_mismatch", "resource_lease.json",
                     proof_id or "")
        return None
    packet.coverage["worker_revision_binding"] = "proof_receipt_resource_lease_join"
    packet.coverage["input/revision-admission.json"] = "not_published_by_worker"
    # The canonical ledger serializer preserves RevisionRef field order.
    return {
        "digest": proof_revision["digest"],
        "semantics": proof_revision["semantics"],
        "reviewed_commit": proof_revision["reviewed_commit"],
    }


def reconcile(root: Path, *, legacy: bool = False, kind: str = "review") -> dict:
    packet = Packet(root)
    verifier = verifier_module()
    binding = None
    worker_proof = None
    worker_lease = None
    if kind == "worker" and not legacy:
        packet.coverage["input/revision-admission.json"] = "not_published_by_worker"
        worker_proof = packet.load("proof_receipt.json", dict,
                                   "ub-review.proof_receipt.v1", required=True)
        worker_lease = packet.load("resource_lease.json", dict,
                                   "ub-review.resource_lease.v1", required=True)
        binding = worker_revision_binding(packet, verifier, worker_proof, worker_lease)
    else:
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
        packet.input_unavailable = True
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
    queue_quantities = {}
    for identity, row in queue.items():
        if row.get("kind") == "sensor" or "lease" in row:
            lease = packet.mapping(row, queue_path, "lease", identity)
            queue_quantities[identity] = packet.lease_quantities(lease, queue_path, identity)
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
    proof_doc = (worker_proof if kind == "worker" and not legacy else
                 packet.load(receipt_path, dict if kind == "worker" else list,
                             required=not legacy))
    raw_proofs = packet.rows([proof_doc] if kind == "worker" and proof_doc is not None else proof_doc, receipt_path)
    proofs = []
    commands: dict[str, tuple[dict, dict]] = {}
    for i, proof in enumerate(raw_proofs):
        identity = proof.get("id")
        if not isinstance(identity, str) or not identity.strip():
            packet.input_unavailable = True
            packet.issue("invalid_identity", receipt_path)
            continue
        if any(not isinstance(proof.get(key, []), list)
               or any(not isinstance(value, str) for value in proof.get(key, []))
               for key in ("request_ids", "requested_by")):
            packet.input_unavailable = True
            packet.issue("invalid_proof_attribution", receipt_path, identity)
            continue
        proofs.append(proof)
        if proof.get("schema") != "ub-review.proof_receipt.v1":
            packet.issue("unsupported_receipt_schema", receipt_path, proof.get("id", ""))
        joined(proof, receipt_path)
        if not proof_result_consistent(proof, kind):
            packet.issue("proof_result_command_mismatch", receipt_path, proof.get("id", ""))
        sides = set()
        if not proof.get("commands"):
            packet.issue("empty_proof_receipt", receipt_path, proof.get("id", ""))
        for j, command in enumerate(packet.rows(proof, receipt_path, "commands")):
            if not integer(command.get("duration_ms"), 128):
                packet.input_unavailable = True
                packet.issue("invalid_command_duration", receipt_path, f"{identity}:{j}")
            side = command.get("side")
            if not isinstance(side, str) or not side:
                packet.input_unavailable = True
                packet.issue("invalid_identity", receipt_path, identity)
                continue
            if side in sides:
                packet.issue("duplicate_or_invalid_side", receipt_path, identity)
            sides.add(side)
            if not isinstance(command.get("status"), str):
                packet.input_unavailable = True
                packet.issue("invalid_command_status", receipt_path, identity)
                continue
            ref = f"{receipt_path}#/commands/{j}" if kind == "worker" else f"{receipt_path}#/{i}/commands/{j}"
            commands[ref] = (proof, command)
    receipt_index = packet.index(proofs, receipt_path)
    receipt_ms = None
    durations = [command.get("duration_ms") for _, command in commands.values()]
    if all(integer(duration, 128) for duration in durations):
        receipt_ms = sum(durations)
        if not integer(receipt_ms, 128):
            packet.input_unavailable = True
            packet.issue("command_duration_sum_overflow", receipt_path)
            receipt_ms = None

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
        packet.input_unavailable = True
        packet.issue("invalid_sensor_inventory", "sensors")

    credited = {}
    executed_proofs = 0
    executed_sensors = 0
    proof_ms = 0
    for task_id, task in ledger.items():
        timing = packet.mapping(task, snapshot_path, "timing", task_id)
        executed = timing.get("process_started_at") is not None
        source = task.get("source")
        if executed:
            if source == "Sensor":
                executed_sensors += 1
            else:
                executed_proofs += 1
                proof_ms += timing.get("process_ms") or 0
        receipt = packet.mapping(task, snapshot_path, "receipt", task_id)
        created = packet.mapping(receipt, snapshot_path, "Created", task_id)
        reference = created.get("reference")
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
                if row.get("status") == "skipped" and not executed and task.get("execution_disposition") == "SetupFailed":
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
        for consumer in packet.rows(task, snapshot_path, "consumers"):
            if consumer.get("requirement") == "Required" and (task.get("execution_disposition") != "Succeeded" or not reference):
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
                required = any(c.get("requirement") == "Required"
                               for c in packet.rows(task, snapshot_path, "consumers"))
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
    lease_doc = (worker_lease if kind == "worker" and not legacy else
                 packet.load(lease_path, dict if kind == "worker" else list,
                             required=not legacy))
    leases = packet.rows([lease_doc] if kind == "worker" and lease_doc is not None else lease_doc, lease_path)
    lease_index = packet.index(leases, lease_path)
    lease_quantities = {}
    for lease in leases:
        lease_quantities[id(lease)] = packet.lease_quantities(lease, lease_path, lease.get("id", ""))
        joined(lease, lease_path)
        if lease.get("schema") != "ub-review.resource_lease.v1":
            packet.issue("unsupported_lease_schema", lease_path, lease.get("id", ""))
        if lease.get("status") == "granted" and lease.get("consumer") not in receipt_index:
            packet.issue("granted_lease_without_receipt", lease_path, lease.get("id", ""))
    for proof in proofs:
        if any(owner is proof and c.get("side") == "head"
               and c.get("status") in {"passed", "failed", "timed_out"}
               for owner, c in commands.values()):
            if not legacy and not any(l.get("consumer") == proof.get("id") and l.get("status") == "granted" for l in leases):
                packet.issue("executed_proof_without_lease", lease_path, proof.get("id", ""))
        if kind == "worker":
            preflight_id = "proof-command-" + verifier.sanitize_artifact_name(proof["id"]) + "-nightly-preflight"
            preflight_task = ledger.get(preflight_id)
            preflight_commands = [(ref, command) for ref, (owner, command) in commands.items()
                                  if owner is proof and command.get("side") == "nightly-preflight"]
            valid_preflight = False
            if preflight_task is not None and len(preflight_commands) == 1:
                preflight_ref, preflight_command = preflight_commands[0]
                timing = packet.mapping(preflight_task, snapshot_path, "timing", preflight_id)
                started = timing.get("process_started_at") is not None
                # A runner failure may publish skipped after setup failure or
                # cancellation. It still requires the real receipted task.
                disposition = {"passed": "Succeeded", "failed": "DeterministicFailure",
                               "timed_out": "TimedOut",
                               "skipped": "Cancelled" if started else "SetupFailed"}.get(preflight_command.get("status"))
                valid_preflight = (
                    preflight_task.get("source") == "Worker"
                    and credited.get(preflight_ref) is preflight_task
                    and disposition is not None
                    and (started or disposition == "SetupFailed")
                    and preflight_task.get("state") == {"ResourcesReleased": disposition}
                    and preflight_task.get("execution_disposition") == disposition
                )
            if not valid_preflight:
                packet.issue("worker_preflight_task_mismatch", snapshot_path, preflight_id)
        if kind == "worker" and proof.get("result") == "skipped_unresolved":
            matching = [lease for lease in leases if lease.get("consumer") == proof.get("id")]
            valid_refusal = (len(matching) == 1
                             and matching[0].get("status") == "refused"
                             and all(matching[0].get(key) == 0
                                     for key in ("cpu", "memory_mb", "disk_mb"))
                             and matching[0].get("scratch") is False
                             and matching[0].get("network") is False)
            if not valid_refusal:
                packet.issue("worker_unresolved_lease_mismatch", lease_path,
                             proof.get("id", ""))
            proof_id = proof.get("id")
            head_task_id = ("proof-command-"
                            + verifier.sanitize_artifact_name(proof_id)
                            + "-head") if isinstance(proof_id, str) else ""
            head_task = ledger.get(head_task_id)
            if (head_task is None
                    or head_task.get("source") != "Worker"
                    or head_task.get("state") != {"TerminallyDeclined": "Refused"}
                    or head_task.get("non_execution_disposition") != "Refused"
                    or head_task.get("execution_disposition") is not None):
                packet.issue("worker_unresolved_head_task_mismatch", snapshot_path,
                             head_task_id or proof.get("id", ""))

    gate_path = "review/gate_outcome.json"
    gate = packet.load(gate_path, dict, "ub-review.gate_outcome.v1", required=kind == "review" and not legacy)
    if gate is not None:
        joined(gate, gate_path)
        required = packet.mapping(gate, gate_path, "required_proof")
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
        runtime = packet.mapping(portfolio, portfolio_path, "runtime")
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
        reservations = {}
        for reservation in packet.rows(task, snapshot_path, "reservations"):
            resource_class = reservation.get("class")
            units = reservation.get("units")
            if not isinstance(resource_class, str) or not integer(units) or units == 0:
                packet.input_unavailable = True
                packet.issue("invalid_reservation", snapshot_path, task.get("id", ""))
                continue
            if resource_class in reservations:
                packet.issue("duplicate_reservation", snapshot_path, task.get("id", ""))
                continue
            reservations[resource_class] = units
        if ref in commands:
            proof, command = commands[ref]
            matches = [l for l in leases if l.get("consumer") == proof.get("id") and l.get("status") == "granted"]
            if len(matches) > 1:
                packet.issue("ambiguous_command_lease", lease_path, proof.get("id", ""))
            if len(matches) == 1 and not (kind == "worker" and command.get("side") != "head"):
                lease = matches[0]
                quantities = lease_quantities[id(lease)]
                if quantities is None:
                    continue
                expected = {cls: quantities[key] for cls, key in
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
            quantities = queue_quantities.get(task["id"])
            if quantities is None:
                continue
            expected = {"Cpu": quantities["cpu"]}
            if quantities["disk_mb"] > 0:
                expected["Disk"] = quantities["disk_mb"]
            timing = packet.mapping(task, snapshot_path, "timing", task["id"])
            if reservations != expected and timing.get("admitted_at") is not None:
                packet.issue("sensor_reservation_mismatch", queue_path, task["id"])

    # Required proof counts must trace to exact source requests and complete
    # receipt outcomes, not command-text similarity or one successful side.
    if gate is not None and not legacy and kind == "review":
        required_requests = [r for r in requests if r.get("required") is True
                             and r.get("lane") == "intelligent-ci-policy"]
        classified = {"matched": len(required_requests), "passed": 0, "failed": 0, "skipped": 0}
        required_projection = packet.mapping(gate, gate_path, "required_proof")
        if required_projection.get("matched") != len(required_requests):
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
                          and proof_result_consistent(p, kind)
                          and p.get("commands")
                          and all(ref in credited for ref, (owner, _) in commands.items() if owner is p)]
            if gate.get("conclusion") == "pass" and not successful:
                packet.issue("pass_without_required_receipt", gate_path, request.get("id", ""))

        if required_projection != classified:
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
            timing = packet.mapping(task, snapshot_path, "timing", task_id)
            if task.get("source") == "Sensor" and timing.get("process_started_at") is not None:
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
    routes_doc = packet.load(routes_path, dict, "ub-review.receipt_routes.v1",
                             required=kind == "review" and not legacy)
    if routes_doc is not None:
        joined(routes_doc, routes_path)
        routes = packet.rows(routes_doc, routes_path, "routes")
        packet.index(routes, routes_path)
        routed_receipts = set()
        for route in routes:
            receipt_id = route.get("receipt_id")
            if not isinstance(receipt_id, str) or not receipt_id.strip():
                packet.input_unavailable = True
                packet.issue("invalid_identity", routes_path, route.get("id", ""))
                continue
            if receipt_id in routed_receipts:
                packet.issue("duplicate_receipt_route", routes_path, receipt_id)
            routed_receipts.add(receipt_id)
            proof = receipt_index.get(receipt_id)
            if proof is None or route.get("result") != proof.get("result"):
                packet.issue("route_receipt_mismatch", routes_path, route.get("id", ""))
            lease_ids = packet.strings(route, routes_path, "lease_ids")
            expected_ids = {identity for identity, lease in lease_index.items()
                            if lease.get("consumer") == receipt_id}
            if len(lease_ids) != len(set(lease_ids)) or set(lease_ids) != expected_ids:
                packet.issue("route_lease_inventory_mismatch", routes_path, receipt_id)
            for lease_id in lease_ids:
                lease = lease_index.get(lease_id)
                if lease is None or lease.get("consumer") != receipt_id:
                    packet.issue("route_lease_mismatch", routes_path, route.get("id", ""))
        for receipt_id in receipt_index.keys() - routed_receipts:
            packet.issue("receipt_without_route", routes_path, receipt_id)

    calibration_path = "review/calibration.json"
    calibration = packet.load(calibration_path, dict, "ub-review.calibration.v0")
    if calibration is not None and ledger:
        reported = packet.mapping(calibration, calibration_path, "counts").get(
            "proof_requests_executed"
        )
        # This v0 field counts ALL proof receipts, including skipped receipts,
        # not executed requests or physical command sides.
        count = len(proofs)
        if reported != count or not integer(reported):
            packet.issue("calibration_proof_count_mismatch", calibration_path)

    metrics_path = "review/metrics.json"
    metrics = packet.load(metrics_path, dict)
    metrics_run = packet.mapping(metrics, metrics_path, "run") if metrics is not None else {}
    if metrics is not None and ledger:
        measured = metrics_run.get("proof_command_duration_ms_sum")
        if measured is not None and not integer(measured, 128):
            packet.input_unavailable = True
            packet.issue("invalid_metric_duration", metrics_path)
        elif measured is not None and receipt_ms is not None and measured != receipt_ms:
            packet.issue("proof_duration_projection_mismatch", metrics_path)
    if scheduler is not None and metrics is not None:
        for key in ("elapsed_wall_ms", "scheduler_roles", "streams", "loops", "phases"):
            if key in scheduler and key in metrics_run and scheduler[key] != metrics_run[key]:
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
    complete = not legacy and binding is not None and packet.coverage.get("task_ledger") == "replay_verified"
    status = "contradictory" if packet.issues or packet.overflow else ("coherent" if complete else "unverifiable")
    report = {"schema": SCHEMA, "packet_kind": kind, "mode": "legacy" if legacy else "current",
              "authority": "shadow-only", "status": status, "revision": binding,
              "issue_count_retained": len(packet.issues), "issues_truncated": packet.overflow,
              "observations_truncated": packet.observations_overflow,
              "input_unavailable": packet.input_unavailable,
              "issues": [{"code": c, "artifact": p, "identity": i} for c, p, i in sorted(packet.issues)],
              "observations": [{"code": c, "artifact": p, "identity": i} for c, p, i in sorted(packet.observations)],
              "counts": {"ledger_tasks": len(ledger) if complete else None,
                         "executed_sensor_tasks": executed_sensors if complete else None,
                         "executed_proof_command_tasks": executed_proofs if complete else None,
                         "proof_receipts": len(proofs) if proof_doc is not None else None,
                         "queue_tasks": len(queue) if queue_doc is not None else None,
                         "portfolio_candidates": len(candidates) if portfolio is not None else None},
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
        if report["input_unavailable"]:
            return 2
        return 0 if report["status"] == "coherent" else 1
    except (OSError, ValueError, KeyError, TypeError, AttributeError, RuntimeError, RecursionError):
        print("task-projection verification failed: input, output, or budget unavailable", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
