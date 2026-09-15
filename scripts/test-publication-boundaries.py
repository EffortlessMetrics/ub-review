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


def revision_identity(semantics: str = "merge_result") -> dict[str, str]:
    base = ("1" * 40, "2" * 40)
    head = ("3" * 40, "4" * 40)
    merge = ("5" * 40, "6" * 40)
    reviewed = head if semantics == "candidate_head" else merge
    canonical_text = "\n".join(
        [
            subject.REVISION_CANONICAL_VERSION,
            f"semantics={semantics}",
            f"base={base[0]} {base[1]}",
            f"head={head[0]} {head[1]}",
            f"reviewed={reviewed[0]} {reviewed[1]}",
            "merge=-" if semantics == "candidate_head" else f"merge={merge[0]} {merge[1]}",
            f"changed_paths={'7' * 64}",
            f"diff={'8' * 64}",
        ]
    ) + "\n"
    digest = hashlib.sha256(
        subject.REVISION_DIGEST_DOMAIN + b"\x00" + canonical_text.encode("utf-8")
    ).hexdigest()
    return {
        "canonical": canonical_text,
        "digest": digest,
        "semantics": semantics,
        "reviewed_commit": reviewed[0],
        "pr_head_commit": head[0],
    }


def base_packet(
    root: Path,
    *,
    prepared: bool = False,
    semantics: str = "merge_result",
) -> dict[str, str]:
    identity = revision_identity(semantics)
    write_json(
        root,
        "input/revision-admission.json",
        {
            "schema": subject.REVISION_ADMISSION_SCHEMA,
            "identity_canonical": identity["canonical"],
            "identity_digest": identity["digest"],
            "semantics": identity["semantics"],
            "reviewed_commit_oid": identity["reviewed_commit"],
            "pr_head_commit": identity["pr_head_commit"],
            "worktree_dirty": False,
        },
    )
    write_json(
        root,
        "review/gate_outcome.json",
        {
            "schema": "ub-review.gate_outcome.v1",
            "revision": {
                "digest": identity["digest"],
                "semantics": identity["semantics"],
                "reviewed_commit": identity["reviewed_commit"],
            },
            "conclusion": "pass",
            "analysis_result": "clean",
            "publication_result": "posted" if prepared else "not_needed",
            "gate_result": "pass",
        },
    )
    payload_status = "prepared" if prepared else "skipped_artifact_only_body"
    write_json(
        root,
        "review/terminal_state.json",
        {
            "schema": "ub-review.terminal_state.v1",
            "status": "sufficient",
            "reviewer_value_present": prepared,
            "review_payload_status": payload_status,
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
                "review_payload_status": payload_status,
                "terminal_state": "sufficient",
                "github_review_json": None,
            },
        )
    return identity


def post_result(root: Path, identity: dict[str, str], *, commit: str | None = None) -> None:
    write_json(root, "review/post-stdout.json", {"id": 7})
    stderr = root / "review/post-stderr.txt"
    stderr.parent.mkdir(parents=True, exist_ok=True)
    stderr.write_text("", encoding="utf-8")
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
            "review_event": "COMMENT",
            "review_body_bytes": len("Material review.".encode("utf-8")),
            "review_comment_count": 0,
            "http_status": 200,
            "token_present": True,
            "payload_written": True,
            "post_stdout_written": True,
            "post_stderr_written": True,
            "response": {
                "id": 7,
                "state": "COMMENTED",
                "commit_id": commit or identity["pr_head_commit"],
            },
        },
    )


def post_error(root: Path, **changes: object) -> None:
    receipt: dict[str, object] = {
        "schema_version": 1,
        "status": "failed",
        "error_kind": "post_failed",
        "failure_stage": "network_post",
    }
    receipt.update(changes)
    write_json(root, "review/post-error.json", receipt)


class PublicationBoundaries(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def report(self) -> dict:
        return subject.reconcile(self.root)

    @staticmethod
    def codes(report: dict) -> set[str]:
        return {row["code"] for row in report["issues"]}

    @staticmethod
    def observations(report: dict) -> set[str]:
        return {row["code"] for row in report["observations"]}

    def test_artifact_only_boundary_is_coherent(self) -> None:
        base_packet(self.root)
        report = self.report()
        self.assertEqual(report["status"], "coherent", report["issues"])
        self.assertEqual(report["preparation_state"], "not_needed")
        self.assertEqual(report["delivery_state"], "not_needed")
        self.assertEqual(report["gate_publication_result"], "not_needed")
        self.assertEqual(report["authority"], "shadow-only")

    def test_candidate_head_identity_is_coherent(self) -> None:
        base_packet(self.root, semantics="candidate_head")
        report = self.report()
        self.assertEqual(report["status"], "coherent", report["issues"])
        self.assertEqual(report["revision"]["semantics"], "candidate_head")
        self.assertEqual(
            report["revision"]["reviewed_commit"],
            report["revision"]["pr_head_commit"],
        )

    def test_prepared_payload_is_not_confirmed_delivery(self) -> None:
        base_packet(self.root, prepared=True)
        report = self.report()
        self.assertEqual(report["delivery_state"], "prepared")
        self.assertIn("prepared_payload_projected_posted", self.codes(report))
        self.assertIn("post_attempt_not_recorded", self.observations(report))

    def test_exact_pr_head_post_is_confirmed(self) -> None:
        identity = base_packet(self.root, prepared=True)
        post_result(self.root, identity)
        report = self.report()
        self.assertEqual(report["status"], "coherent", report["issues"])
        self.assertEqual(report["delivery_state"], "confirmed")

    def test_stale_or_wrong_post_head_is_rejected(self) -> None:
        identity = base_packet(self.root, prepared=True)
        post_result(self.root, identity, commit="9" * 40)
        report = self.report()
        self.assertEqual(report["status"], "contradictory")
        self.assertEqual(report["delivery_state"], "failed")
        self.assertIn("post_response_head_mismatch", self.codes(report))

    def test_post_error_cannot_remain_projected_as_posted(self) -> None:
        base_packet(self.root, prepared=True)
        post_error(self.root)
        report = self.report()
        self.assertEqual(report["delivery_state"], "failed")
        self.assertIn("failed_delivery_projected_posted", self.codes(report))

    def test_success_without_response_head_is_unverifiable(self) -> None:
        identity = base_packet(self.root, prepared=True)
        post_result(self.root, identity)
        result = read_json(self.root, "review/post-result.json")
        result["response"].pop("commit_id")
        write_json(self.root, "review/post-result.json", result)
        report = self.report()
        self.assertEqual(report["status"], "contradictory", report["issues"])
        self.assertEqual(report["delivery_state"], "unverifiable")
        self.assertIn("unverifiable_delivery_projected_posted", self.codes(report))
        self.assertIn("post_response_head_unavailable", self.observations(report))

    def test_invalid_success_receipt_never_confirms(self) -> None:
        identity = base_packet(self.root, prepared=True)
        post_result(self.root, identity)
        result = read_json(self.root, "review/post-result.json")
        result["http_status"] = 500
        write_json(self.root, "review/post-result.json", result)
        report = self.report()
        self.assertTrue(report["input_unavailable"])
        self.assertEqual(report["delivery_state"], "unverifiable")
        self.assertIn("post_success_http_status_invalid", self.codes(report))

    def test_success_receipt_must_bind_prepared_payload(self) -> None:
        identity = base_packet(self.root, prepared=True)
        post_result(self.root, identity)
        result = read_json(self.root, "review/post-result.json")
        result["review_body_bytes"] += 1
        result["review_comment_count"] = 1
        write_json(self.root, "review/post-result.json", result)
        report = self.report()
        self.assertTrue(report["input_unavailable"])
        self.assertEqual(report["delivery_state"], "unverifiable")
        self.assertIn("post_success_payload_mismatch", self.codes(report))

    def test_success_receipt_requires_terminal_log_files(self) -> None:
        identity = base_packet(self.root, prepared=True)
        post_result(self.root, identity)
        (self.root / "review/post-stderr.txt").unlink()
        report = self.report()
        self.assertTrue(report["input_unavailable"])
        self.assertEqual(report["delivery_state"], "unverifiable")
        self.assertIn("post_stderr_missing", self.codes(report))

    def test_success_receipt_requires_commented_response_state(self) -> None:
        identity = base_packet(self.root, prepared=True)
        post_result(self.root, identity)
        result = read_json(self.root, "review/post-result.json")
        result["response"]["state"] = "PENDING"
        write_json(self.root, "review/post-result.json", result)
        report = self.report()
        self.assertTrue(report["input_unavailable"])
        self.assertEqual(report["delivery_state"], "unverifiable")
        self.assertIn("invalid_post_response_state", self.codes(report))

    def test_invalid_error_receipt_is_unverifiable(self) -> None:
        base_packet(self.root, prepared=True)
        post_error(self.root, failure_stage="")
        report = self.report()
        self.assertTrue(report["input_unavailable"])
        self.assertEqual(report["delivery_state"], "unverifiable")
        self.assertIn("invalid_post_failure_stage", self.codes(report))

    def test_malformed_review_is_not_prepared(self) -> None:
        base_packet(self.root, prepared=True)
        write_json(self.root, "review/github-review.json", {"body": 7, "comments": []})
        report = self.report()
        self.assertEqual(report["preparation_state"], "unverifiable")
        self.assertEqual(report["delivery_state"], "unverifiable")
        self.assertIn("invalid_github_review", self.codes(report))

    def test_malformed_skip_is_not_not_needed(self) -> None:
        base_packet(self.root)
        write_json(self.root, "review/github-review-skip.json", {})
        report = self.report()
        self.assertEqual(report["preparation_state"], "unverifiable")
        self.assertEqual(report["delivery_state"], "unverifiable")
        self.assertIn("invalid_github_review_skip", self.codes(report))

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

    def test_skip_post_error_is_failed_and_contradictory(self) -> None:
        base_packet(self.root)
        post_error(self.root)
        report = self.report()
        self.assertEqual(report["status"], "contradictory")
        self.assertEqual(report["delivery_state"], "failed")
        self.assertIn("post_error_present_for_skip", self.codes(report))

    def test_terminal_payload_must_match_prepared_surface(self) -> None:
        base_packet(self.root, prepared=True)
        terminal = read_json(self.root, "review/terminal_state.json")
        terminal["review_payload_status"] = "skipped_pass_policy"
        write_json(self.root, "review/terminal_state.json", terminal)
        report = self.report()
        self.assertEqual(report["status"], "contradictory")
        self.assertIn("terminal_payload_mismatch", self.codes(report))

    def test_terminal_payload_and_status_must_match_skip(self) -> None:
        base_packet(self.root)
        terminal = read_json(self.root, "review/terminal_state.json")
        terminal["review_payload_status"] = "skipped_pass_policy"
        terminal["status"] = "artifact-only"
        write_json(self.root, "review/terminal_state.json", terminal)
        report = self.report()
        self.assertIn("terminal_payload_mismatch", self.codes(report))
        self.assertIn("terminal_status_mismatch", self.codes(report))

    def test_forged_revision_digest_is_rejected(self) -> None:
        base_packet(self.root)
        admission = read_json(self.root, "input/revision-admission.json")
        admission["identity_digest"] = "a" * 64
        write_json(self.root, "input/revision-admission.json", admission)
        report = self.report()
        self.assertTrue(report["input_unavailable"])
        self.assertIsNone(report["revision"])
        self.assertIn("revision_digest_mismatch", self.codes(report))

    def test_exposed_revision_fields_must_match_canonical(self) -> None:
        base_packet(self.root)
        admission = read_json(self.root, "input/revision-admission.json")
        admission["semantics"] = "candidate_head"
        admission["reviewed_commit_oid"] = "9" * 40
        admission["pr_head_commit"] = "a" * 40
        write_json(self.root, "input/revision-admission.json", admission)
        report = self.report()
        codes = self.codes(report)
        self.assertIn("revision_semantics_mismatch", codes)
        self.assertIn("reviewed_commit_mismatch", codes)
        self.assertIn("pr_head_commit_mismatch", codes)

    def test_non_normalized_canonical_identity_is_rejected(self) -> None:
        base_packet(self.root)
        admission = read_json(self.root, "input/revision-admission.json")
        admission["identity_canonical"] = admission["identity_canonical"].replace("\n", "\r\n")
        write_json(self.root, "input/revision-admission.json", admission)
        report = self.report()
        self.assertIn("invalid_revision_canonical", self.codes(report))

    def test_review_and_post_receipts_are_xor_surfaces(self) -> None:
        identity = base_packet(self.root, prepared=True)
        write_json(
            self.root,
            "review/github-review-skip.json",
            {
                "schema_version": 1,
                "status": "skipped",
                "review_payload_status": "skipped_pass_policy",
                "terminal_state": "sufficient",
                "github_review_json": None,
            },
        )
        post_result(self.root, identity)
        post_error(self.root)
        report = self.report()
        codes = self.codes(report)
        self.assertIn("prepared_review_xor_violation", codes)
        self.assertIn("post_receipt_xor_violation", codes)
        self.assertEqual(report["preparation_state"], "unverifiable")

    def test_duplicate_json_keys_fail_closed(self) -> None:
        base_packet(self.root)
        (self.root / "review/gate_outcome.json").write_bytes(
            b'{"schema":"ub-review.gate_outcome.v1","schema":"other"}'
        )
        report = self.report()
        self.assertTrue(report["input_unavailable"])
        self.assertEqual(report["status"], "unverifiable")
        self.assertIn("invalid_publication_artifact", self.codes(report))

    def test_missing_required_input_is_unverifiable(self) -> None:
        base_packet(self.root)
        (self.root / "input/revision-admission.json").unlink()
        report = self.report()
        self.assertTrue(report["input_unavailable"])
        self.assertEqual(report["status"], "unverifiable")
        self.assertIn("missing_publication_artifact", self.codes(report))

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
