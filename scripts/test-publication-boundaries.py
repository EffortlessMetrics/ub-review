#!/usr/bin/env python3
"""Executable #957 publication-boundary reconciliation regressions."""
from __future__ import annotations

import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location(
    "publication_boundaries",
    HERE / "reconcile-publication-boundaries.py",
)
assert SPEC is not None and SPEC.loader is not None
subject = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(subject)


def write_json(root: Path, path: str, value: object) -> None:
    target = root / path
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_bytes(subject.canonical(value))


def read_json(root: Path, path: str) -> object:
    return json.loads((root / path).read_bytes())


def base_packet(root: Path, *, prepared: bool = False) -> dict[str, str]:
    digest = "a" * 64
    reviewed = "b" * 40
    pr_head = "c" * 40
    binding = {
        "digest": digest,
        "semantics": "merge_result",
        "reviewed_commit": reviewed,
        "pr_head_commit": pr_head,
    }
    write_json(
        root,
        "input/revision-admission.json",
        {
            "schema": "ub-review.revision_admission.v1",
            "identity_digest": digest,
            "semantics": "merge_result",
            "reviewed_commit_oid": reviewed,
            "pr_head_commit": pr_head,
        },
    )
    write_json(
        root,
        "review/gate_outcome.json",
        {
            "schema": "ub-review.gate_outcome.v1",
            "revision": {
                "digest": digest,
                "semantics": "merge_result",
                "reviewed_commit": reviewed,
            },
            "conclusion": "pass",
            "analysis_result": "clean",
            "publication_result": "posted" if prepared else "not_needed",
            "gate_result": "pass",
        },
    )
    write_json(
        root,
        "review/terminal_state.json",
        {
            "schema": "ub-review.terminal_state.v1",
            "status": "sufficient",
            "reviewer_value_present": prepared,
        },
    )
    if prepared:
        write_json(
            root,
            "review/github-review.json",
            {"event": "COMMENT", "body": "Material review.", "comments": []},
        )
    else:
        write_json(
            root,
            "review/github-review-skip.json",
            {
                "schema_version": 1,
                "status": "skipped",
                "reason": "artifact-only",
                "review_payload_status": "skipped_artifact_only_body",
                "terminal_state": "sufficient",
                "github_review_json": None,
            },
        )
    return binding


def post_result(root: Path, binding: dict[str, str], *, commit: str | None = None) -> None:
    write_json(root, "review/post-stdout.json", {"id": 7})
    write_json(
        root,
        "review/post-result.json",
        {
            "schema_version": 1,
            "status": "ok",
            "repo": "EffortlessMetrics/ub-review",
            "repo_valid": True,
            "pull_number": 1,
            "comments": 0,
            "review_json": "target/ub-review/review/github-review.json",
            "review_json_exists": True,
            "review_json_valid": True,
            "http_status": 200,
            "token_present": True,
            "payload_written": True,
            "post_stdout_written": True,
            "post_stderr_written": False,
            "response": {
                "id": 7,
                "commit_id": commit or binding["pr_head_commit"],
            },
        },
    )


class PublicationBoundaries(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def report(self) -> dict:
        return subject.reconcile(self.root)

    def codes(self) -> set[str]:
        return {row["code"] for row in self.report()["issues"]}

    def test_artifact_only_boundary_is_coherent(self) -> None:
        base_packet(self.root)
        report = self.report()
        self.assertEqual(report["status"], "coherent", report["issues"])
        self.assertEqual(report["preparation_state"], "not_needed")
        self.assertEqual(report["delivery_state"], "not_needed")
        self.assertEqual(report["gate_publication_result"], "not_needed")
        self.assertEqual(report["authority"], "shadow-only")

    def test_prepared_payload_is_not_confirmed_delivery(self) -> None:
        base_packet(self.root, prepared=True)
        report = self.report()
        self.assertEqual(report["delivery_state"], "prepared")
        self.assertIn("prepared_payload_projected_posted", self.codes())
        self.assertIn(
            "post_attempt_not_recorded",
            {row["code"] for row in report["observations"]},
        )

    def test_exact_pr_head_post_is_confirmed(self) -> None:
        binding = base_packet(self.root, prepared=True)
        post_result(self.root, binding)
        report = self.report()
        self.assertEqual(report["status"], "coherent", report["issues"])
        self.assertEqual(report["delivery_state"], "confirmed")

    def test_stale_or_wrong_post_head_is_rejected(self) -> None:
        binding = base_packet(self.root, prepared=True)
        post_result(self.root, binding, commit="d" * 40)
        report = self.report()
        self.assertEqual(report["status"], "contradictory")
        self.assertIn("post_response_head_mismatch", self.codes())

    def test_post_error_cannot_remain_projected_as_posted(self) -> None:
        base_packet(self.root, prepared=True)
        write_json(
            self.root,
            "review/post-error.json",
            {
                "schema_version": 1,
                "status": "failed",
                "error_kind": "missing_token",
                "failure_stage": "preflight",
            },
        )
        report = self.report()
        self.assertEqual(report["delivery_state"], "failed")
        self.assertIn("failed_delivery_projected_posted", self.codes())

    def test_review_and_post_receipts_are_xor_surfaces(self) -> None:
        binding = base_packet(self.root, prepared=True)
        write_json(
            self.root,
            "review/github-review-skip.json",
            {
                "schema_version": 1,
                "status": "skipped",
                "review_payload_status": "skipped_pass_policy",
                "github_review_json": None,
            },
        )
        post_result(self.root, binding)
        write_json(
            self.root,
            "review/post-error.json",
            {
                "schema_version": 1,
                "status": "failed",
                "error_kind": "post_failed",
                "failure_stage": "network_post",
            },
        )
        codes = self.codes()
        self.assertIn("prepared_review_xor_violation", codes)
        self.assertIn("post_receipt_xor_violation", codes)

    def test_missing_and_invalid_inputs_fail_closed(self) -> None:
        base_packet(self.root)
        (self.root / "review/github-review-skip.json").unlink()
        self.assertIn("prepared_review_xor_violation", self.codes())
        write_json(
            self.root,
            "review/github-review-skip.json",
            {
                "schema_version": 1,
                "status": "skipped",
                "review_payload_status": "skipped_artifact_only_body",
                "github_review_json": None,
            },
        )
        (self.root / "review/gate_outcome.json").write_bytes(
            b'{"schema":"ub-review.gate_outcome.v1","schema":"other"}'
        )
        report = self.report()
        self.assertTrue(report["input_unavailable"])
        self.assertEqual(report["status"], "unverifiable")
        self.assertIn("invalid_publication_artifact", self.codes())

    def test_symlink_input_is_rejected(self) -> None:
        base_packet(self.root)
        target = self.root / "review/github-review-skip.json"
        target.unlink()
        target.symlink_to(self.root / "review/gate_outcome.json")
        completed = subprocess.run(
            [sys.executable, str(HERE / "reconcile-publication-boundaries.py"), str(self.root)],
            capture_output=True,
            timeout=10,
        )
        self.assertEqual(completed.returncode, 2)

    def test_skip_post_result_is_accepted(self) -> None:
        base_packet(self.root)
        write_json(
            self.root,
            "review/post-result.json",
            {"schema_version": 1, "status": "skipped", "reason": "artifact-only"},
        )
        report = self.report()
        self.assertEqual(report["status"], "coherent", report["issues"])
        self.assertEqual(report["delivery_state"], "not_needed")

    def test_success_without_response_head_is_unverifiable(self) -> None:
        binding = base_packet(self.root, prepared=True)
        post_result(self.root, binding)
        result = read_json(self.root, "review/post-result.json")
        result["response"].pop("commit_id")
        write_json(self.root, "review/post-result.json", result)
        report = self.report()
        self.assertEqual(report["status"], "unverifiable", report["issues"])
        self.assertEqual(report["delivery_state"], "unverifiable")
        self.assertIn(
            "post_response_head_unavailable",
            {row["code"] for row in report["observations"]},
        )

    def test_missing_required_input_is_unverifiable(self) -> None:
        base_packet(self.root)
        (self.root / "input/revision-admission.json").unlink()
        report = self.report()
        self.assertTrue(report["input_unavailable"])
        self.assertEqual(report["status"], "unverifiable")
        self.assertIn("missing_publication_artifact", self.codes())

    def test_report_is_deterministic_bounded_and_atomic(self) -> None:
        base_packet(self.root)
        first = self.report()
        second = self.report()
        self.assertEqual(subject.canonical(first), subject.canonical(second))
        for source in first["source_artifacts"]:
            data = (self.root / source["path"]).read_bytes()
            self.assertEqual(source["bytes"], len(data))
            self.assertEqual(source["sha256"], hashlib.sha256(data).hexdigest())
        subject.publish_report(self.root, first)
        report_path = self.root / subject.REPORT_PATH
        self.assertEqual(report_path.read_bytes(), subject.canonical(first))
        self.assertLessEqual(len(report_path.read_bytes()), subject.MAX_REPORT_BYTES)
        self.assertFalse(list((self.root / "review").glob(".publication-boundary-*")))

    def test_cli_replaces_stale_report_and_uses_stable_exit_codes(self) -> None:
        base_packet(self.root)
        write_json(self.root, subject.REPORT_PATH, {"schema": subject.SCHEMA, "stale": True})
        completed = subprocess.run(
            [
                sys.executable,
                str(HERE / "reconcile-publication-boundaries.py"),
                str(self.root),
                "--write-report",
            ],
            capture_output=True,
            timeout=10,
        )
        self.assertEqual(completed.returncode, 0, completed.stderr)
        report = json.loads(completed.stdout)
        self.assertEqual(report["status"], "coherent")
        self.assertEqual(read_json(self.root, subject.REPORT_PATH), report)


if __name__ == "__main__":
    unittest.main()
