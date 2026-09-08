#!/usr/bin/env python3
"""Executable #957 reconciliation regressions; no provider/network/process proofs."""
from __future__ import annotations

import copy
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("task_projections", HERE / "reconcile-task-projections.py")
assert spec is not None and spec.loader is not None
subject = importlib.util.module_from_spec(spec)
spec.loader.exec_module(subject)
verifier = subject.verifier_module()


def write_json(root: Path, path: str, value: object) -> None:
    target = root / path
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_bytes(subject.canonical(value))


def read_json(root: Path, path: str) -> object:
    return json.loads((root / path).read_bytes())


def coherent(root: Path, *, model_on: bool = False, worker: bool = False) -> tuple[dict, list]:
    canonical = ("ub-review.revision-identity.v1\nsemantics=candidate_head\n"
                 f"base={'b' * 40} {'c' * 40}\nhead={'d' * 40} {'e' * 40}\n"
                 f"reviewed={'d' * 40} {'e' * 40}\nmerge=-\n"
                 f"changed_paths={'f' * 64}\ndiff={'a' * 64}\n")
    digest = hashlib.sha256(b"ub-review.revision-identity.digest.v1\x00" + canonical.encode()).hexdigest()
    binding = {"digest": digest, "semantics": "candidate_head", "reviewed_commit": "d" * 40}
    if not worker:
        write_json(root, "input/revision-admission.json", {
            "schema": "ub-review.revision_admission.v1", "identity_canonical": canonical,
            "identity_digest": digest, "semantics": "candidate_head",
            "reviewed_commit_oid": "d" * 40, "pr_head_commit": "d" * 40,
            "worktree_dirty": False})
    head_command = {"side": "head", "command": "cargo test --locked --test selected", "env": {},
                    "status": "passed", "exit_code": 0, "timed_out": False,
                    "timeout_sec": 60, "duration_ms": 10,
                    "stdout": "proof/proof-a/head/stdout.txt",
                    "stderr": "proof/proof-a/head/stderr.txt", "reason": "completed"}
    commands = [head_command]
    if worker:
        commands.insert(0, {"side": "nightly-preflight", "command": "cargo +nightly --version",
                            "env": {}, "status": "passed", "exit_code": 0,
                            "timed_out": False, "timeout_sec": 60, "duration_ms": 2,
                            "stdout": "proof/proof-a/nightly-preflight/stdout.txt",
                            "stderr": "proof/proof-a/nightly-preflight/stderr.txt",
                            "reason": "completed"})
    proof = {"schema": "ub-review.proof_receipt.v1", "id": "proof-a", "kind": "focused-test" if worker else "focused-head",
             "base": "b" * 40, "head": "d" * 40, "revision": binding,
             "test_patch_mode": "head-only", "requested_by": ["model" if model_on else "impact-planner"],
             "request_ids": [] if worker else ["request-a"], "result": "passed" if worker else "head_passed", "reason": "completed",
             "commands": commands}
    proof_path = "proof_receipt.json" if worker else "review/proof_receipts.json"
    head_ref = proof_path + ("#/commands/1" if worker else "#/0/commands/0")
    write_json(root, proof_path, proof if worker else [proof])
    lease = {"schema": "ub-review.resource_lease.v1", "id": "lease-proof-a", "kind": "focused-test",
             "consumer": "proof-a", "status": "granted", "revision": binding,
             "cpu": 1, "memory_mb": 8, "disk_mb": 8, "network": False,
             "scratch": False, "timeout_sec": 60, "reason": "test"}
    write_json(root, "resource_lease.json" if worker else "review/resource_leases.json", lease if worker else [lease])
    source = "Worker" if worker else ({"ReviewerTurn": {"model_on": True}} if model_on else "Required")
    events = []

    def executed(task_id: str, task_source: object, reference: str, reservations: list,
                 required: bool, offset: int) -> None:
        for event in [
            {"Proposed": {"revision": binding, "source": task_source, "limits": {"timeout_ceiling_ms": 60000}}},
            {"ConsumerAttached": {"consumer": {"id": "gate" if required else "reviewer",
                                               "requirement": "Required" if required else "Optional",
                                               "value": "GateCritical" if required else "Advisory"}}},
            "Selected", {"Queued": {"at": offset}},
            {"Admitted": {"at": offset + 1, "reservations": reservations}},
            {"SetupStarted": {"at": offset + 2}}, {"RunStarted": {"at": offset + 3}},
            {"ProcessFinished": {"at": offset + 13, "disposition": "Succeeded"}},
            {"CleanupFinished": {"at": offset + 14}},
            {"ReceiptCreated": {"at": offset + 15, "reference": reference}},
            {"ResourcesReleased": {"at": offset + 16}},
        ]:
            events.append((task_id, event))

    head_reservations = [{"class": "Cpu", "units": 1}, {"class": "Memory", "units": 8},
                         {"class": "Disk", "units": 8}, {"class": "Test", "units": 1}]
    if worker:
        executed("proof-command-proof-a-nightly-preflight", source,
                 "proof_receipt.json#/commands/0",
                 [{"class": "Cpu", "units": 1}, {"class": "Test", "units": 1}],
                 False, 1)
        executed("proof-command-proof-a-head", source, head_ref, head_reservations,
                 False, 20)
    else:
        executed("proof-command-proof-a-head", source, head_ref, head_reservations,
                 not model_on, 1)
        events.extend([
            ("proof-request-request-a", {"Proposed": {"revision": binding, "source": source, "limits": {"timeout_ceiling_ms": 60000}}}),
            ("proof-request-request-a", {"ConsumerAttached": {"consumer": {"id": "gate", "requirement": "Required" if not model_on else "Optional", "value": "GateCritical" if not model_on else "Advisory"}}}),
            ("proof-request-request-a", {"TerminallyDeclined": {"at": 30, "disposition": "Superseded", "reason": "grouped into proof-a", "existing_receipt": None}}),
        ])
        sensor_path = "sensors/check/ub-review-sensor-status.json"
        executed("sensor-check", "Sensor", sensor_path, [{"class": "Cpu", "units": 1}, {"class": "Disk", "units": 8}], True, 31)
        write_json(root, sensor_path, {"sensor": "check", "status": "ok", "required": True,
                                      "duration_ms": 10, "timeout_sec": 60, "exit_code": 0})
        write_json(root, "work_queue.json", {"schema": "ub-review.work_queue.v1", "tasks": [
            {"id": "sensor-check", "kind": "sensor", "status": "completed", "receipt_path": sensor_path,
             "lease": {"cpu": 1, "memory_mb": 0, "disk_mb": 8, "timeout_sec": 60}},
            {"id": "proof-a", "kind": "focused-head", "status": "completed", "receipt_path": proof_path}]})
        write_json(root, "review/proof_portfolio.json", {"schema": "ub-review.proof_portfolio.v1", "head": "d" * 40,
                   "budget_seconds": 60, "candidate_count": 1,
                   "candidate_tasks": [{"id": "proof-a", "kind": "focused-head", "required": not model_on}],
                   "decisions": [{"task_id": "proof-a", "status": "selected"}], "selected_task_ids": ["proof-a"]})
        write_json(root, "review/proof_requests.json", [{"schema": "ub-review.proof_request.v1", "id": "request-a", "lane": "model" if model_on else "intelligent-ci-policy", "required": not model_on}])
        write_json(root, "review/gate_outcome.json", {"schema": "ub-review.gate_outcome.v1", "revision": binding,
                   "conclusion": "pass", "gate_result": "pass",
                   "required_proof": {"matched": int(not model_on), "passed": int(not model_on), "failed": 0, "skipped": 0}})
        write_json(root, "review/receipt_routes.json", {"schema": "ub-review.receipt_routes.v1", "revision": binding,
                   "routes": [{"id": "route-a", "receipt_id": "proof-a", "result": "head_passed", "lease_ids": ["lease-proof-a"]}]})
        write_json(root, "review/calibration.json", {"schema": "ub-review.calibration.v0", "counts": {"proof_requests_executed": 1}})
        write_json(root, "review/fill-ledger.json", {"schema": "ub-review.fill_ledger.v1", "entries": [{"check_id": "check", "kind": "sensor", "selected": True}]})
        write_json(root, "review/metrics.json", {"schema_version": 1, "run": {"proof_command_duration_ms_sum": 10, "elapsed_wall_ms": 47}})
        write_json(root, "review/scheduler.json", {"schema": "ub-review.scheduler.v1", "elapsed_wall_ms": 47})
    seal(root, binding, events)
    return binding, events


def seal(root: Path, binding: dict, events: list) -> None:
    event_bytes, snapshot_bytes = verifier._task_ledger_self_test_artifacts(events, binding)
    (root / "task_ledger_events.ndjson").write_bytes(event_bytes)
    (root / "review").mkdir(exist_ok=True)
    (root / "review/task_ledger_snapshot.json").write_bytes(snapshot_bytes)


class Projections(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.binding, self.events = coherent(self.root)

    def report(self):
        return subject.reconcile(self.root)

    def codes(self):
        return {x["code"] for x in self.report()["issues"]}

    def change(self, name, mutate):
        value = read_json(self.root, name)
        mutate(value)
        write_json(self.root, name, value)

    def test_coherent_model_off_on_and_worker(self):
        for model_on, worker in [(False, False), (True, False), (False, True)]:
            with self.subTest(model_on=model_on, worker=worker), tempfile.TemporaryDirectory() as td:
                root = Path(td)
                coherent(root, model_on=model_on, worker=worker)
                report = subject.reconcile(root, kind="worker" if worker else "review")
                self.assertEqual(report["status"], "coherent", report["issues"])
                self.assertEqual(report["counts"]["executed_proof_command_tasks"],
                                 2 if worker else 1)
                self.assertEqual(report["authority"], "shadow-only")

    def test_deterministic_and_read_only(self):
        before = {str(p.relative_to(self.root)): p.read_bytes() for p in self.root.rglob("*") if p.is_file()}
        self.assertEqual(subject.canonical(self.report()), subject.canonical(self.report()))
        after = {str(p.relative_to(self.root)): p.read_bytes() for p in self.root.rglob("*") if p.is_file()}
        self.assertEqual(before, after)
        for source in self.report()["source_artifacts"]:
            data = (self.root / source["path"]).read_bytes()
            self.assertEqual(source["bytes"], len(data))
            self.assertEqual(source["sha256"], hashlib.sha256(data).hexdigest())

    def test_portfolio_head_must_bind_to_admitted_commit(self):
        path = "review/proof_portfolio.json"
        self.change(path, lambda row: row.update(head="a" * 40))
        result = subprocess.run([sys.executable, str(HERE / "reconcile-task-projections.py"),
                                 str(self.root)], capture_output=True, timeout=10)
        self.assertEqual(result.returncode, 1, result.stderr)
        report = json.loads(result.stdout)
        self.assertEqual(report["status"], "contradictory")
        self.assertFalse(report["input_unavailable"])
        self.assertIn("portfolio_revision_mismatch", {row["code"] for row in report["issues"]})
        for head in [None, "HEAD", "d" * 39, 7]:
            with self.subTest(head=head):
                self.change(path, lambda row: row.update(head=head))
                report = self.report()
                self.assertTrue(report["input_unavailable"])
                self.assertIn("portfolio_revision_unavailable", {row["code"] for row in report["issues"]})
        self.change(path, lambda row: row.update(head=self.binding["reviewed_commit"]))
        self.assertEqual(self.report()["status"], "coherent")

    def test_current_missing_planes_never_pass(self):
        for name in ["input/revision-admission.json", "task_ledger_events.ndjson", "review/task_ledger_snapshot.json",
                     "work_queue.json", "review/proof_portfolio.json", "review/proof_receipts.json",
                     "review/resource_leases.json", "review/gate_outcome.json", "review/proof_requests.json",
                     "review/receipt_routes.json"]:
            with self.subTest(name=name):
                path = self.root / name
                old = path.read_bytes()
                path.unlink()
                self.assertNotEqual(self.report()["status"], "coherent")
                path.write_bytes(old)

    def test_planned_sensor_and_missing_impact(self):
        self.change("work_queue.json", lambda x: x["tasks"][0].update(status="planned"))
        self.assertIn("successful_sensor_left_planned", self.codes())
        self.change("work_queue.json", lambda x: x["tasks"].pop())
        self.change("review/proof_portfolio.json", lambda x: x.update(candidate_tasks=[], decisions=[], selected_task_ids=[], candidate_count=0))
        self.assertIn("executed_impact_receipt_missing_from_queue_and_portfolio", self.codes())

    def test_receipt_without_task(self):
        seal(self.root, self.binding, [(t, e) for t, e in self.events if t != "proof-command-proof-a-head"])
        self.assertIn("receipt_without_task", self.codes())

    def test_missing_release_rejected_by_canonical_replay(self):
        seal(self.root, self.binding, [(t, e) for t, e in self.events if not (t == "proof-command-proof-a-head" and isinstance(e, dict) and "ResourcesReleased" in e)])
        self.assertIn("unreleased_resources", self.codes())

    def test_forged_snapshot_rejected(self):
        self.change("review/task_ledger_snapshot.json", lambda x: x["tasks"][0].update(resources_released=False))
        self.assertIn("forged_snapshot", self.codes())

    def test_digest_corruption_and_reordering(self):
        path = self.root / "task_ledger_events.ndjson"
        original = path.read_bytes()
        lines = original.splitlines(keepends=True)
        path.write_bytes(b"".join([lines[1], lines[0], *lines[2:]]))
        self.assertIn("event_order", self.codes())
        record = json.loads(lines[0])
        record["digest"] = "0" * 64
        path.write_bytes(verifier._task_ledger_json(record) + b"\n" + b"".join(lines[1:]))
        self.assertIn("source_digest_mismatch", self.codes())

    def test_wrong_revision_and_side_and_terminal(self):
        self.change("review/proof_receipts.json", lambda x: x[0]["revision"].update(reviewed_commit="c" * 40))
        self.assertIn("revision_mismatch", self.codes())
        self.change("review/proof_receipts.json", lambda x: x[0]["commands"][0].update(side="base-plus-tests", status="failed"))
        self.assertIn("receipt_task_identity_mismatch", self.codes())
        self.assertIn("conflicting_terminal", self.codes())

    def test_duplicate_and_wrong_lease(self):
        self.change("review/resource_leases.json", lambda x: x.append(copy.deepcopy(x[0])))
        self.assertIn("duplicate_identity", self.codes())
        self.assertIn("ambiguous_command_lease", self.codes())
        self.change("review/resource_leases.json", lambda x: x.pop())
        self.change("review/resource_leases.json", lambda x: x[0].update(cpu=9))
        self.assertIn("lease_reservation_mismatch", self.codes())

    def test_routes_and_counts(self):
        self.change("review/receipt_routes.json", lambda x: x["routes"][0].update(result="head_failed", lease_ids=["absent"]))
        self.assertIn("route_receipt_mismatch", self.codes())
        self.assertIn("route_lease_mismatch", self.codes())
        self.change("review/calibration.json", lambda x: x["counts"].update(proof_requests_executed=99))
        self.assertIn("calibration_proof_count_mismatch", self.codes())
        self.change("review/scheduler.json", lambda x: x.update(elapsed_wall_ms=999))
        self.assertIn("scheduler_metric_mismatch", self.codes())
        self.change("review/metrics.json", lambda x: x["run"].update(proof_command_duration_ms_sum=0))
        self.assertIn("proof_duration_projection_mismatch", self.codes())

    def test_receipt_routes_require_complete_unique_inventories(self):
        path = "review/receipt_routes.json"
        original = read_json(self.root, path)
        cases = [
            ("missing route", "receipt_without_route",
             lambda rows: rows.clear()),
            ("missing lease", "route_lease_inventory_mismatch",
             lambda rows: rows[0].update(lease_ids=[])),
            ("duplicate lease", "route_lease_inventory_mismatch",
             lambda rows: rows[0].update(lease_ids=["lease-proof-a", "lease-proof-a"])),
            ("extra route", "route_receipt_mismatch",
             lambda rows: rows.append(dict(rows[0], id="extra", receipt_id="absent"))),
            ("duplicate receipt route", "duplicate_receipt_route",
             lambda rows: rows.append(dict(rows[0], id="duplicate"))),
        ]
        for name, code, mutate in cases:
            with self.subTest(name=name):
                document = copy.deepcopy(original)
                mutate(document["routes"])
                write_json(self.root, path, document)
                report = self.report()
                self.assertEqual(report["status"], "contradictory")
                self.assertFalse(report["input_unavailable"])
                self.assertIn(code, {row["code"] for row in report["issues"]})
        write_json(self.root, path, original)

    def test_receipt_routes_include_refused_leases_without_order_authority(self):
        leases = read_json(self.root, "review/resource_leases.json")
        leases.append(dict(leases[0], id="lease-refused", status="refused",
                           cpu=0, memory_mb=0, disk_mb=0))
        write_json(self.root, "review/resource_leases.json", leases)
        self.assertIn("route_lease_inventory_mismatch", self.codes())
        self.change("review/receipt_routes.json", lambda row: row["routes"][0].update(
            lease_ids=["lease-refused", "lease-proof-a"]))
        report = self.report()
        self.assertEqual(report["status"], "coherent", report["issues"])

    def test_lease_quantities_match_unsigned_rust_domains(self):
        original = {"cpu": 1, "memory_mb": 8, "disk_mb": 8, "timeout_sec": 60}
        for key in original:
            maximum = (1 << (32 if key == "cpu" else 64)) - 1
            for value in [None, "1", True, -1, 1.5, [], {}, maximum + 1]:
                with self.subTest(key=key, value=value):
                    packet = subject.Packet(self.root)
                    lease = dict(original, **{key: value})
                    self.assertIsNone(packet.lease_quantities(lease, "lease", "test"))
                    self.assertTrue(packet.input_unavailable)
                    self.assertIn(("invalid_lease_quantity", "lease", f"test:{key}"), packet.issues)
            missing = dict(original)
            missing.pop(key)
            packet = subject.Packet(self.root)
            self.assertIsNone(packet.lease_quantities(missing, "lease", "test"))
            for value in [0, maximum]:
                with self.subTest(key=key, valid_value=value):
                    packet = subject.Packet(self.root)
                    lease = dict(original, **{key: value})
                    self.assertEqual(packet.lease_quantities(lease, "lease", "test"), lease)
                    self.assertFalse(packet.input_unavailable)

    def test_malformed_lease_cli_retains_json_and_replaces_stale_report(self):
        for surface in ["review", "worker", "sensor"]:
            with self.subTest(surface=surface), tempfile.TemporaryDirectory() as td:
                root = Path(td)
                coherent(root, worker=surface == "worker")
                if surface == "worker":
                    path = "resource_lease.json"
                    document = read_json(root, path)
                    document["memory_mb"] = []
                    proof = read_json(root, "proof_receipt.json")
                    proof["head"] = "c" * 40
                    write_json(root, "proof_receipt.json", proof)
                    independent = "worker_revision_head_mismatch"
                else:
                    queue = read_json(root, "work_queue.json")
                    queue["tasks"][0]["status"] = "planned"
                    write_json(root, "work_queue.json", queue)
                    independent = "successful_sensor_left_planned"
                    path = "work_queue.json" if surface == "sensor" else "review/resource_leases.json"
                    document = read_json(root, path)
                    lease = document["tasks"][0]["lease"] if surface == "sensor" else document[0]
                    lease["disk_mb"] = "invalid"
                write_json(root, path, document)
                command = [sys.executable, str(HERE / "reconcile-task-projections.py"), str(root)]
                if surface == "worker":
                    command.extend(["--kind", "worker"])
                for publish in [False, True]:
                    with self.subTest(publish=publish):
                        stale = {"schema": subject.SCHEMA, "status": "coherent", "stale": True}
                        write_json(root, subject.REPORT_PATH, stale)
                        result = subprocess.run([*command, *(["--write-report"] if publish else [])],
                                                capture_output=True, timeout=10)
                        self.assertEqual(result.returncode, 2, result.stderr)
                        report = json.loads(result.stdout)
                        self.assertTrue(report["input_unavailable"])
                        codes = {row["code"] for row in report["issues"]}
                        self.assertIn("invalid_lease_quantity", codes)
                        self.assertIn(independent, codes)
                        self.assertLessEqual(len(result.stdout), subject.MAX_REPORT_BYTES)
                        self.assertEqual(read_json(root, subject.REPORT_PATH), report if publish else stale)

    def test_required_missing_link_under_pass(self):
        self.change("review/proof_receipts.json", lambda x: x[0].update(request_ids=[]))
        self.assertIn("pass_without_required_receipt", self.codes())

    def test_accounting_coherence_is_not_a_product_pass(self):
        # A correctly accounted deterministic failure is coherent, not clean.
        changed = []
        for task, event in self.events:
            event = copy.deepcopy(event)
            if task == "proof-command-proof-a-head" and isinstance(event, dict) and "ProcessFinished" in event:
                event["ProcessFinished"]["disposition"] = "DeterministicFailure"
            changed.append((task, event))
        seal(self.root, self.binding, changed)
        self.change("review/proof_receipts.json", lambda x: (x[0].update(result="head_failed"), x[0]["commands"][0].update(status="failed", exit_code=1)))
        self.change("review/receipt_routes.json", lambda x: x["routes"][0].update(result="head_failed"))
        self.change("review/gate_outcome.json", lambda x: x.update(conclusion="fail", gate_result="finding", required_proof={"matched": 1, "passed": 0, "failed": 1, "skipped": 0}))
        report = self.report()
        self.assertEqual(report["status"], "coherent", report["issues"])
        self.assertIn("required_proof_unproven", {x["code"] for x in report["observations"]})

    def test_malformed_json_and_boolean_counts(self):
        (self.root / "review/calibration.json").write_bytes(b'{"schema":"ub-review.calibration.v0","schema":"other"}')
        self.assertIn("invalid_projection", self.codes())
        self.change("review/gate_outcome.json", lambda x: x["required_proof"].update(matched=True))
        self.assertIn("required_count_mismatch", self.codes())

    def test_source_budget_and_symlink_refusal(self):
        with patch.object(subject, "MAX_FILE_BYTES", 16):
            self.assertNotEqual(self.report()["status"], "coherent")
        target = self.root / "review/calibration.json"
        target.unlink()
        target.symlink_to(self.root / "review/gate_outcome.json")
        self.assertIn("invalid_projection", self.codes())

    def test_issue_output_is_bounded(self):
        packet = subject.Packet(self.root)
        for index in range(1000):
            packet.issue("test", "x" * 10000, str(index))
        self.assertEqual(len(packet.issues), subject.MAX_ISSUES)
        self.assertTrue(packet.overflow)
        self.assertTrue(all(len(row[1]) <= 120 for row in packet.issues))

    def test_report_publication_and_stale_invalidation(self):
        report = self.report()
        subject.publish_report(self.root, report)
        self.assertEqual((self.root / subject.REPORT_PATH).read_bytes(), subject.canonical(report))
        self.assertFalse(list((self.root / "review").glob(".task-projections-*")))
        (self.root / "review/proof_receipts.json").write_text('null')
        completed = subprocess.run([sys.executable, str(HERE / "reconcile-task-projections.py"), str(self.root), "--write-report"], capture_output=True, timeout=10)
        self.assertEqual(completed.returncode, 2)
        self.assertNotEqual(read_json(self.root, subject.REPORT_PATH)["status"], "coherent")

    def test_model_requiredness_never_becomes_gate_policy(self):
        self.change("review/proof_requests.json", lambda x: x[0].update(lane="model", required=True))
        self.change("review/gate_outcome.json", lambda x: x.update(required_proof={"matched": 0, "passed": 0, "failed": 0, "skipped": 0}))
        self.assertEqual(self.report()["status"], "coherent", self.report()["issues"])

    def test_required_counts_are_cross_checked_even_under_fail(self):
        self.change("review/gate_outcome.json", lambda x: x.update(conclusion="fail", required_proof={"matched": 1, "passed": 0, "failed": 1, "skipped": 0}))
        self.assertIn("required_receipt_count_mismatch", self.codes())

    def test_similar_commands_are_not_deduplicated(self):
        self.change("review/proof_receipts.json", lambda x: x.append(copy.deepcopy(x[0])))
        report = self.report()
        self.assertIn("duplicate_identity", {x["code"] for x in report["issues"]})
        self.assertEqual(len(report["similar_command_candidates"]), 1)
        self.assertEqual(report["similar_command_candidates"][0]["equivalence"], "unproven")
        self.assertEqual(report["counts"]["proof_receipts"], 2)

    def test_receipt_and_ledger_duration_are_not_conflated(self):
        self.change("review/proof_receipts.json", lambda x: x[0]["commands"][0].update(duration_ms=12))
        self.change("review/metrics.json", lambda x: x["run"].update(proof_command_duration_ms_sum=12))
        self.assertEqual(self.report()["status"], "coherent", self.report()["issues"])
        self.assertIn("receipt_ledger_duration_domains_differ", {x["code"] for x in self.report()["observations"]})

    def test_malformed_command_duration_cannot_match_zero_metrics(self):
        path = "review/proof_receipts.json"
        original = read_json(self.root, path)
        self.change("review/metrics.json", lambda row: row["run"].update(proof_command_duration_ms_sum=0))
        for value in [None, True, -1, 1.5, "0", [], {}, 1 << 128, "missing"]:
            with self.subTest(value=value):
                proof = copy.deepcopy(original)
                command = proof[0]["commands"][0]
                if value == "missing":
                    command.pop("duration_ms")
                else:
                    command["duration_ms"] = value
                write_json(self.root, path, proof)
                stale = {"schema": subject.SCHEMA, "status": "coherent", "stale": True}
                write_json(self.root, subject.REPORT_PATH, stale)
                result = subprocess.run([sys.executable, str(HERE / "reconcile-task-projections.py"),
                                         str(self.root), "--write-report"], capture_output=True, timeout=10)
                self.assertEqual(result.returncode, 2, result.stderr)
                report = json.loads(result.stdout)
                self.assertTrue(report["input_unavailable"])
                self.assertIn("invalid_command_duration", {row["code"] for row in report["issues"]})
                self.assertEqual(read_json(self.root, subject.REPORT_PATH), report)

    def test_command_duration_uses_full_u128_domain(self):
        for duration in [0, 1 << 64, (1 << 128) - 1]:
            with self.subTest(duration=duration):
                self.change("review/proof_receipts.json", lambda rows: rows[0]["commands"][0].update(duration_ms=duration))
                self.change("review/metrics.json", lambda row: row["run"].update(proof_command_duration_ms_sum=duration))
                report = self.report()
                self.assertFalse(report["input_unavailable"])
                self.assertEqual(report["status"], "coherent", report["issues"])

    def test_duration_validation_does_not_require_metrics(self):
        (self.root / "review/metrics.json").unlink()
        self.change("review/proof_receipts.json", lambda rows: rows[0]["commands"][0].update(duration_ms=None))
        report = self.report()
        self.assertTrue(report["input_unavailable"])
        self.assertIn("invalid_command_duration", {row["code"] for row in report["issues"]})
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            coherent(root, worker=True)
            proof = read_json(root, "proof_receipt.json")
            for command in proof["commands"]:
                command["duration_ms"] = (1 << 128) - 1
            write_json(root, "proof_receipt.json", proof)
            report = subject.reconcile(root, kind="worker")
            self.assertTrue(report["input_unavailable"])
            self.assertIn("command_duration_sum_overflow", {row["code"] for row in report["issues"]})

    def test_atomic_publish_failure_removes_temporary_file(self):
        with patch.object(subject.os, "replace", side_effect=OSError("injected rename failure")):
            with self.assertRaises(OSError):
                subject.publish_report(self.root, self.report())
        self.assertFalse((self.root / subject.REPORT_PATH).exists())
        self.assertFalse(list((self.root / "review").glob(".task-projections-*")))

    def test_incomplete_input_cannot_leave_old_coherent_report(self):
        subject.publish_report(self.root, self.report())
        (self.root / "sensors").rename(self.root / "sensor-backup")
        (self.root / "sensors").symlink_to(self.root / "sensor-backup", target_is_directory=True)
        result = subprocess.run([sys.executable, str(HERE / "reconcile-task-projections.py"), str(self.root), "--write-report"], capture_output=True, timeout=10)
        self.assertEqual(result.returncode, 2)
        self.assertFalse((self.root / subject.REPORT_PATH).exists())

    def test_classification_matches_current_rust_authority(self):
        source = (HERE.parent / "src/gate.rs").read_text()
        self.assertIn("request.required && request.lane == REQUIRED_PROOF_POLICY_LANE", source)
        self.assertIn('"head_passed" | "discriminating" => RequiredProofClass::Passed', source)
        self.assertIn('"head_failed" | "timed_out" => RequiredProofClass::Failed', source)
        calibration = (HERE.parent / "src/calibration.rs").read_text()
        self.assertIn("let proof_requests_executed = proof_receipts.len();", calibration)

    def test_candidate_requiredness_and_deferred_queue(self):
        self.change("review/proof_portfolio.json", lambda x: x["candidate_tasks"][0].update(required=False))
        self.assertIn("candidate_requiredness_mismatch", self.codes())
        self.change("review/proof_portfolio.json", lambda x: x["decisions"][0].update(status="deferred_by_budget"))
        self.change("work_queue.json", lambda x: x["tasks"][1].update(status="planned"))
        self.assertIn("terminal_portfolio_task_left_planned", self.codes())

    def test_empty_receipt_cannot_satisfy_required_work(self):
        self.change("review/proof_receipts.json", lambda x: x[0].update(commands=[]))
        self.assertIn("empty_proof_receipt", self.codes())
        self.assertIn("pass_without_required_receipt", self.codes())

    def test_existing_receipt_link_must_resolve(self):
        events = copy.deepcopy(self.events)
        events.extend([
            ("reused", {"Proposed": {"revision": self.binding, "source": "Impact", "limits": {"timeout_ceiling_ms": 60000}}}),
            ("reused", {"TerminallyDeclined": {"at": 99, "disposition": "SatisfiedByExistingReceipt", "reason": "exact retained receipt", "existing_receipt": "review/proof_receipts.json#/0/commands/0"}}),
        ])
        seal(self.root, self.binding, events)
        self.assertEqual(self.report()["status"], "coherent", self.report()["issues"])
        events[-1][1]["TerminallyDeclined"]["existing_receipt"] = "review/proof_receipts.json#/9/commands/0"
        seal(self.root, self.binding, events)
        self.assertIn("existing_receipt_missing", self.codes())

    def test_setup_failure_is_not_a_cancelled_process(self):
        events = []
        for task_id, event in copy.deepcopy(self.events):
            if task_id == "proof-command-proof-a-head" and isinstance(event, dict):
                if "RunStarted" in event or "CleanupFinished" in event:
                    continue
                if "ProcessFinished" in event:
                    event = {"SetupFailed": {"at": 14}}
            events.append((task_id, event))
        seal(self.root, self.binding, events)
        self.change("review/proof_receipts.json", lambda rows: rows[0].update(result="skipped_profile"))
        self.change("review/proof_receipts.json", lambda rows: rows[0]["commands"][0].update(status="skipped", exit_code=None, duration_ms=0))
        self.change("review/gate_outcome.json", lambda row: row.update(conclusion="inconclusive", gate_result="not_proven", required_proof={"matched": 1, "passed": 0, "failed": 0, "skipped": 1}))
        self.change("review/receipt_routes.json", lambda row: row["routes"][0].update(result="skipped_profile"))
        self.change("review/metrics.json", lambda row: row["run"].update(proof_command_duration_ms_sum=0))
        report = self.report()
        self.assertEqual(report["status"], "coherent", report["issues"])
        self.assertEqual(report["counts"]["executed_proof_command_tasks"], 0)
        self.assertIn("required_consumer_unsatisfied", {row["code"] for row in report["observations"]})

    def test_worker_preflight_has_separate_task_not_an_invented_lease(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            binding, events = coherent(root, worker=True)
            report = subject.reconcile(root, kind="worker")
            self.assertEqual(report["status"], "coherent", report["issues"])
            self.assertEqual(report["counts"]["executed_proof_command_tasks"], 2)
            self.assertEqual(report["coverage"]["worker_preflight_lease"], "not_separately_published")
            self.assertEqual(report["coverage"]["worker_revision_binding"],
                             "proof_receipt_resource_lease_join")
            self.assertEqual(report["coverage"]["input/revision-admission.json"],
                             "not_published_by_worker")
            proof = read_json(root, "proof_receipt.json")
            proof["commands"][0].update(status="skipped", exit_code=None, duration_ms=0)
            write_json(root, "proof_receipt.json", proof)
            seal(root, binding, [event for event in events if event[0] != "proof-command-proof-a-nightly-preflight"])
            report = subject.reconcile(root, kind="worker")
            self.assertIn("worker_preflight_task_mismatch", {row["code"] for row in report["issues"]})

    def test_worker_binding_rejects_receipt_lease_or_head_disagreement(self):
        for code in ["worker_revision_mismatch", "worker_revision_head_mismatch"]:
            with self.subTest(code=code), tempfile.TemporaryDirectory() as td:
                root = Path(td)
                coherent(root, worker=True)
                path = "resource_lease.json" if code == "worker_revision_mismatch" else "proof_receipt.json"
                value = read_json(root, path)
                if code == "worker_revision_mismatch":
                    value["revision"]["reviewed_commit"] = "c" * 40
                else:
                    value["head"] = "c" * 40
                write_json(root, path, value)
                report = subject.reconcile(root, kind="worker")
                self.assertNotEqual(report["status"], "coherent")
                self.assertIn(code, {row["code"] for row in report["issues"]})

    def test_unresolved_worker_matches_production_result_and_refused_lease(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            binding, events = coherent(root, worker=True)
            proof = read_json(root, "proof_receipt.json")
            proof["result"] = "skipped_unresolved"
            proof["commands"][1].update(status="skipped", exit_code=None,
                                         duration_ms=0,
                                         reason="typed proof intent unresolved")
            write_json(root, "proof_receipt.json", proof)
            lease = read_json(root, "resource_lease.json")
            lease.update(status="refused", cpu=0, memory_mb=0, disk_mb=0,
                         scratch=False,
                         reason="executor adapter could not resolve the typed proof intent")
            write_json(root, "resource_lease.json", lease)

            head_task = "proof-command-proof-a-head"
            unresolved_events = []
            for task_id, event in events:
                if task_id != head_task:
                    unresolved_events.append((task_id, event))
                elif isinstance(event, dict) and (
                        "Proposed" in event or "ConsumerAttached" in event):
                    unresolved_events.append((task_id, event))
            unresolved_events.append((head_task, {
                "TerminallyDeclined": {
                    "at": 50,
                    "disposition": "Refused",
                    "reason": "executor adapter could not resolve the typed proof intent",
                    "existing_receipt": None,
                }
            }))
            seal(root, binding, unresolved_events)

            report = subject.reconcile(root, kind="worker")
            self.assertEqual(report["status"], "coherent", report["issues"])
            self.assertEqual(report["counts"]["executed_proof_command_tasks"], 1)

            # Unavailable preflight is still a real task: the producer can
            # publish a skipped receipt after setup failure or cancellation.
            preflight_task = "proof-command-proof-a-nightly-preflight"
            for status, disposition in [("failed", "DeterministicFailure"),
                                        ("timed_out", "TimedOut"),
                                        ("skipped", "SetupFailed"),
                                        ("skipped", "Cancelled")]:
                with self.subTest(preflight=status, disposition=disposition):
                    preflight_events = []
                    for task_id, event in copy.deepcopy(unresolved_events):
                        if task_id == preflight_task and isinstance(event, dict):
                            if disposition == "SetupFailed" and ("RunStarted" in event or "CleanupFinished" in event):
                                continue
                            if "ProcessFinished" in event:
                                event = ({"SetupFailed": {"at": 14}} if disposition == "SetupFailed"
                                         else {"ProcessFinished": {"at": 14, "disposition": disposition}})
                        preflight_events.append((task_id, event))
                    seal(root, binding, preflight_events)
                    proof["commands"][0].update(status=status, exit_code=1 if status == "failed" else None,
                                                 timed_out=status == "timed_out", duration_ms=0)
                    write_json(root, "proof_receipt.json", proof)
                    report = subject.reconcile(root, kind="worker")
                    self.assertEqual(report["status"], "coherent", report["issues"])
                    if status == "skipped":
                        proof["commands"][0]["status"] = "passed"
                        write_json(root, "proof_receipt.json", proof)
                        forged = subject.reconcile(root, kind="worker")
                        self.assertIn("worker_preflight_task_mismatch", {row["code"] for row in forged["issues"]})
                        proof["commands"][0]["status"] = status
                        write_json(root, "proof_receipt.json", proof)

            no_preflight = [row for row in unresolved_events if row[0] != preflight_task]
            seal(root, binding, no_preflight)
            report = subject.reconcile(root, kind="worker")
            self.assertIn("worker_preflight_task_mismatch", {row["code"] for row in report["issues"]})
            proof["commands"][0].update(status="passed", exit_code=0, timed_out=False, duration_ms=2)
            write_json(root, "proof_receipt.json", proof)
            seal(root, binding, unresolved_events)

            missing_head_events = [row for row in unresolved_events
                                   if row[0] != head_task]
            seal(root, binding, missing_head_events)
            report = subject.reconcile(root, kind="worker")
            self.assertIn("worker_unresolved_head_task_mismatch",
                          {row["code"] for row in report["issues"]})
            seal(root, binding, unresolved_events)

            lease["status"] = "granted"
            write_json(root, "resource_lease.json", lease)
            report = subject.reconcile(root, kind="worker")
            self.assertIn("worker_unresolved_lease_mismatch",
                          {row["code"] for row in report["issues"]})

            lease["status"] = "refused"
            write_json(root, "resource_lease.json", lease)
            proof["commands"][1]["status"] = "passed"
            write_json(root, "proof_receipt.json", proof)
            report = subject.reconcile(root, kind="worker")
            self.assertIn("proof_result_command_mismatch",
                          {row["code"] for row in report["issues"]})

    def test_legacy_mode_never_grants_current_coherence(self):
        self.assertEqual(subject.reconcile(self.root, legacy=True)["status"], "unverifiable")

    def test_discriminating_claim_requires_the_base_witness(self):
        self.change("review/proof_receipts.json", lambda rows: rows[0].update(result="discriminating"))
        self.change("review/receipt_routes.json", lambda row: row["routes"][0].update(result="discriminating"))
        self.assertIn("proof_result_command_mismatch", self.codes())
        self.assertIn("pass_without_required_receipt", self.codes())

    def test_result_classification_side_matrix(self):
        proof = read_json(self.root, "review/proof_receipts.json")[0]
        proof.update(kind="focused-red-green", test_patch_mode="base-plus-tests")
        base = copy.deepcopy(proof["commands"][0])
        base["side"] = "base-plus-tests"
        proof["commands"].append(base)
        for status, results in [("failed", {"discriminating"}), ("passed", {"non_discriminating"}), ("timed_out", {"timed_out"}), ("skipped", {"base_patch_failed", "skipped_profile"})]:
            base["status"] = status
            for result in ["head_passed", "head_failed", "discriminating", "non_discriminating", "timed_out", "base_patch_failed", "skipped_profile"]:
                with self.subTest(status=status, result=result):
                    proof["result"] = result
                    self.assertEqual(subject.proof_result_consistent(proof, "review"), result in results)

    def test_missing_projection_count_is_unknown_not_zero(self):
        (self.root / "review/proof_receipts.json").unlink()
        (self.root / "work_queue.json").unlink()
        (self.root / "review/proof_portfolio.json").unlink()
        counts = self.report()["counts"]
        for key in ["proof_receipts", "queue_tasks", "portfolio_candidates"]:
            self.assertIsNone(counts[key])

    def test_rejected_bytes_still_consume_the_aggregate_read_budget(self):
        for name in ["a", "b", "c"]:
            (self.root / name).write_bytes(b"x" * 32)
        packet = subject.Packet(self.root)
        with patch.object(subject, "MAX_FILE_BYTES", 8), patch.object(subject, "MAX_INPUT_BYTES", 16):
            for name in ["a", "b", "c"]:
                with self.assertRaises(subject.PacketError):
                    packet.read(name)
            # One extra byte is the probe that establishes an exceeded limit.
            self.assertEqual(packet.total, 17)
            self.assertEqual(len(packet.raw), 3)

    def test_observation_overflow_is_not_a_contradiction(self):
        class ManyObservations(subject.Packet):
            def __init__(self, root):
                super().__init__(root)
                for i in range(subject.MAX_ISSUES + 1):
                    self.observe("measured_duration_difference", "test", str(i))
        with patch.object(subject, "Packet", ManyObservations):
            report = self.report()
        self.assertEqual(report["status"], "coherent", report["issues"])
        self.assertTrue(report["observations_truncated"])
        self.assertFalse(report["issues_truncated"])
        self.assertEqual(report["issue_count_retained"], 0)

    def test_cli_exit_codes_distinguish_invalid_input_and_contradictions(self):
        path = self.root / "review/calibration.json"
        original = path.read_bytes()
        for data in [b"{", b"null", b'[]', b'{"schema":"unsupported"}']:
            with self.subTest(data=data):
                path.write_bytes(data)
                run = subprocess.run([sys.executable, str(HERE / "reconcile-task-projections.py"), str(self.root)], capture_output=True, timeout=10)
                self.assertEqual(run.returncode, 2)
                self.assertTrue(json.loads(run.stdout)["input_unavailable"])
        path.write_bytes(original)
        self.change("work_queue.json", lambda row: row["tasks"][0].update(status="planned"))
        run = subprocess.run([sys.executable, str(HERE / "reconcile-task-projections.py"), str(self.root)], capture_output=True, timeout=10)
        self.assertEqual(run.returncode, 1)
        self.assertFalse(json.loads(run.stdout)["input_unavailable"])

    def test_malformed_proof_fields_do_not_suppress_other_findings(self):
        name = "review/proof_receipts.json"
        original = read_json(self.root, name)
        self.change("work_queue.json", lambda row: row["tasks"][0].update(status="planned"))
        cases = [("id", None), ("id", []), ("id", {}), ("id", 1),
                 ("commands", None), ("commands", "bad"), ("commands", [None]),
                 ("commands", [{"side": None, "status": "passed"}]),
                 ("commands", [{"side": [], "status": "passed"}]),
                 ("commands", [{"side": "head", "status": []}]),
                 ("requested_by", None), ("request_ids", [None])]
        for key, value in cases:
            with self.subTest(key=key, value=value):
                rows = copy.deepcopy(original)
                rows[0][key] = value
                write_json(self.root, name, rows)
                report = self.report()
                self.assertTrue(report["input_unavailable"])
                self.assertNotEqual(report["status"], "coherent")
                self.assertIn("successful_sensor_left_planned", {x["code"] for x in report["issues"]})
                result = subprocess.run([sys.executable, str(HERE / "reconcile-task-projections.py"),
                                         str(self.root)], capture_output=True, timeout=10)
                self.assertEqual(result.returncode, 2)
                self.assertNotIn(b"Traceback", result.stderr)
                self.assertTrue(json.loads(result.stdout)["input_unavailable"])
        write_json(self.root, name, original)

    def test_malformed_nested_collections_emit_bounded_json_and_replace_stale_report(self):
        self.change("work_queue.json",
                    lambda row: row["tasks"][0].update(status="planned"))
        self.change("review/receipt_routes.json",
                    lambda row: row["routes"][0].update(lease_ids=None))

        command = [sys.executable, str(HERE / "reconcile-task-projections.py"),
                   str(self.root)]
        result = subprocess.run(command, capture_output=True, timeout=10)
        self.assertEqual(result.returncode, 2)
        self.assertNotIn(b"Traceback", result.stderr)
        report = json.loads(result.stdout)
        codes = {row["code"] for row in report["issues"]}
        self.assertTrue(report["input_unavailable"])
        self.assertIn("invalid_projection_rows", codes)
        self.assertIn("successful_sensor_left_planned", codes)

        stale = {"schema": subject.SCHEMA, "status": "coherent", "stale": True}
        write_json(self.root, subject.REPORT_PATH, stale)
        written = subprocess.run([*command, "--write-report"],
                                 capture_output=True, timeout=10)
        self.assertEqual(written.returncode, 2)
        replacement = read_json(self.root, subject.REPORT_PATH)
        self.assertNotEqual(replacement, stale)
        self.assertTrue(replacement["input_unavailable"])
        self.assertIn("successful_sensor_left_planned",
                      {row["code"] for row in replacement["issues"]})

    def test_nested_collection_helpers_fail_closed_without_raising(self):
        packet = subject.Packet(self.root)
        self.assertEqual(packet.rows({"consumers": None}, "snapshot", "consumers"), [])
        self.assertEqual(packet.rows({"reservations": "bad"}, "snapshot", "reservations"), [])
        self.assertEqual(packet.strings({"lease_ids": None}, "routes", "lease_ids"), [])
        self.assertEqual(packet.mapping({"run": []}, "metrics", "run"), {})
        self.assertTrue(packet.input_unavailable)
        self.assertEqual(
            {row[0] for row in packet.issues},
            {"invalid_projection_rows", "invalid_projection_object"},
        )

    def test_historical_incident_corpus_preserves_every_expected_violation(self):
        corpus = HERE.parent / "fixtures/authority-incidents"
        manifest = json.loads((corpus / "manifest.json").read_bytes())
        total = 0
        for case in manifest["cases"]:
            with self.subTest(case=case["id"]):
                for source in case["files"]:
                    data = (corpus / source["path"]).read_bytes()
                    total += len(data)
                    self.assertEqual(len(data), source["bytes"])
                    self.assertEqual(hashlib.sha256(data).hexdigest(), source["sha256"])
                result = subject.reconcile(corpus / case["id"], legacy=True)
                codes = {x["code"] for x in result["issues"] + result["observations"]}
                self.assertNotEqual(result["status"], "coherent")
                self.assertIsNone(result["counts"]["ledger_tasks"])
                self.assertTrue({x["code"] for x in case["expected_violations"]} <= codes, codes)
        self.assertLessEqual(total, manifest["max_total_bytes"])


if __name__ == "__main__":
    unittest.main()
