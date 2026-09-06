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
    write_json(root, "input/revision-admission.json", {
        "schema": "ub-review.revision_admission.v1", "identity_canonical": canonical,
        "identity_digest": digest, "semantics": "candidate_head",
        "reviewed_commit_oid": "d" * 40, "pr_head_commit": "d" * 40, "worktree_dirty": False})
    proof = {"schema": "ub-review.proof_receipt.v1", "id": "proof-a", "kind": "focused-head",
             "base": "b" * 40, "head": "d" * 40, "revision": binding,
             "test_patch_mode": "head-only", "requested_by": ["model" if model_on else "impact-planner"],
             "request_ids": [] if worker else ["request-a"], "result": "head_passed", "reason": "completed",
             "commands": [{"side": "head", "command": "cargo test --locked --test selected", "env": {},
                           "status": "passed", "exit_code": 0, "timed_out": False,
                           "timeout_sec": 60, "duration_ms": 10,
                           "stdout": "proof/proof-a/head/stdout.txt", "stderr": "proof/proof-a/head/stderr.txt",
                           "reason": "completed"}]}
    proof_path = "proof_receipt.json" if worker else "review/proof_receipts.json"
    proof_ref = proof_path + ("#/commands/0" if worker else "#/0/commands/0")
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

    executed("proof-command-proof-a-head", source, proof_ref,
             [{"class": "Cpu", "units": 1}, {"class": "Memory", "units": 8},
              {"class": "Disk", "units": 8}, {"class": "Test", "units": 1}], not model_on and not worker, 1)
    if not worker:
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
             "lease": {"cpu": 1, "disk_mb": 8}},
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
                self.assertEqual(report["counts"]["executed_proof_command_tasks"], 1)
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

    def test_current_missing_planes_never_pass(self):
        for name in ["input/revision-admission.json", "task_ledger_events.ndjson", "review/task_ledger_snapshot.json",
                     "work_queue.json", "review/proof_portfolio.json", "review/proof_receipts.json",
                     "review/resource_leases.json", "review/gate_outcome.json", "review/proof_requests.json"]:
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
        self.assertEqual(completed.returncode, 1)
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
            proof = read_json(root, "proof_receipt.json")
            preflight = copy.deepcopy(proof["commands"][0])
            preflight.update(side="nightly-preflight", command="cargo +nightly --version")
            proof["commands"].insert(0, preflight)
            proof["result"] = "passed"
            write_json(root, "proof_receipt.json", proof)
            preflight_events = []
            for _, event in copy.deepcopy(events):
                if isinstance(event, dict) and "Admitted" in event:
                    event["Admitted"]["reservations"] = [{"class": "Cpu", "units": 1}, {"class": "Test", "units": 1}]
                preflight_events.append(("proof-command-proof-a-nightly-preflight", event))
            for _, event in events:
                if isinstance(event, dict) and "ReceiptCreated" in event:
                    event["ReceiptCreated"]["reference"] = "proof_receipt.json#/commands/1"
            seal(root, binding, preflight_events + events)
            report = subject.reconcile(root, kind="worker")
            self.assertEqual(report["status"], "coherent", report["issues"])
            self.assertEqual(report["counts"]["executed_proof_command_tasks"], 2)
            self.assertEqual(report["coverage"]["worker_preflight_lease"], "not_separately_published")

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
