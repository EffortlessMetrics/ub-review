#!/usr/bin/env python3
"""Reconcile prepared review and post receipts without changing product authority.

This #957 checker is deliberately read-only and shadow-only. It binds the
prepared review/skip surface, post success/error receipts, and the run-stage
gate publication projection to one admitted revision. It does not finalize the
product outcome; #959/#960 own that authority migration.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import sys
import tempfile
from typing import Any

SCHEMA = "ub-review.publication_boundary_reconciliation.v1"
REPORT_PATH = "review/publication_boundary_reconciliation.json"
MAX_FILE_BYTES = 4 * 1024 * 1024
MAX_INPUT_BYTES = 16 * 1024 * 1024
MAX_INPUT_FILES = 32
MAX_ISSUES = 128
MAX_REPORT_BYTES = 256 * 1024
OID_RE = re.compile(r"[0-9a-f]{40}|[0-9a-f]{64}")
SHA256_RE = re.compile(r"[0-9a-f]{64}")
SKIP_STATUSES = {
    "skipped_empty_smoke",
    "skipped_artifact_only_body",
    "skipped_pass_policy",
    "skipped_gate_failure_artifact_only",
}


def canonical(value: Any) -> bytes:
    return (
        json.dumps(
            value,
            sort_keys=True,
            separators=(",", ":"),
            ensure_ascii=False,
            allow_nan=False,
        )
        + "\n"
    ).encode("utf-8")


def unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate JSON key")
        result[key] = value
    return result


def strict_json(data: bytes) -> Any:
    def reject_constant(value: str) -> None:
        raise ValueError(f"non-finite JSON number: {value}")

    return json.loads(
        data,
        object_pairs_hook=unique_object,
        parse_constant=reject_constant,
    )


def safe_label(value: Any) -> str:
    if not isinstance(value, str):
        return "invalid-identity"
    printable = "".join(character if character.isprintable() else "?" for character in value)
    if len(printable) <= 120:
        return printable
    digest = hashlib.sha256(value.encode("utf-8", errors="replace")).hexdigest()[:16]
    return f"{printable[:96]}~{digest}"


class PacketError(ValueError):
    pass


class Packet:
    def __init__(self, root: Path):
        self.root = root.resolve(strict=True)
        if not self.root.is_dir():
            raise PacketError("packet root is not a directory")
        self.raw: dict[str, bytes | None] = {}
        self.sources: dict[str, dict[str, Any]] = {}
        self.total_bytes = 0
        self.input_unavailable = False
        self.issues: set[tuple[str, str, str]] = set()
        self.observations: set[tuple[str, str, str]] = set()
        self.issues_truncated = False
        self.observations_truncated = False

    def issue(self, code: str, artifact: str, identity: str = "") -> None:
        row = (safe_label(code), safe_label(artifact), safe_label(identity))
        if row in self.issues:
            return
        if len(self.issues) >= MAX_ISSUES:
            self.issues_truncated = True
            return
        self.issues.add(row)

    def observe(self, code: str, artifact: str, identity: str = "") -> None:
        row = (safe_label(code), safe_label(artifact), safe_label(identity))
        if row in self.observations:
            return
        if len(self.observations) >= MAX_ISSUES:
            self.observations_truncated = True
            return
        self.observations.add(row)

    def path(self, name: str) -> Path:
        relative = PurePosixPath(name)
        if (
            not name
            or relative.is_absolute()
            or ".." in relative.parts
            or "\\" in name
            or str(relative) != name
            or "\x00" in name
        ):
            raise PacketError("unsafe packet path")
        path = self.root
        for part in relative.parts:
            path = path / part
            if path.is_symlink():
                raise PacketError("symlink packet input")
        return path

    def read(self, name: str, *, required: bool = False) -> bytes | None:
        if name in self.raw:
            data = self.raw[name]
            if required and data is None:
                self.input_unavailable = True
                self.issue("missing_publication_artifact", name)
            return data
        if len(self.raw) >= MAX_INPUT_FILES:
            raise PacketError("input file budget exceeded")
        path = self.path(name)
        if not path.exists():
            self.raw[name] = None
            if required:
                self.input_unavailable = True
                self.issue("missing_publication_artifact", name)
            return None
        if not path.is_file():
            raise PacketError("packet input is not a regular file")
        remaining = MAX_INPUT_BYTES - self.total_bytes
        if remaining <= 0:
            raise PacketError("input byte budget exceeded")
        with path.open("rb") as stream:
            data = stream.read(min(MAX_FILE_BYTES, remaining) + 1)
        if len(data) > MAX_FILE_BYTES:
            raise PacketError("per-file byte budget exceeded")
        self.total_bytes += len(data)
        if self.total_bytes > MAX_INPUT_BYTES:
            raise PacketError("input byte budget exceeded")
        self.raw[name] = data
        self.sources[name] = {
            "path": name,
            "bytes": len(data),
            "sha256": hashlib.sha256(data).hexdigest(),
        }
        return data

    def load(self, name: str, *, required: bool = False) -> Any:
        try:
            data = self.read(name, required=required)
            if data is None:
                return None
            return strict_json(data)
        except (OSError, ValueError, RecursionError):
            self.input_unavailable = True
            self.issue("invalid_publication_artifact", name)
            return None


def valid_oid(value: Any) -> bool:
    return isinstance(value, str) and OID_RE.fullmatch(value) is not None


def revision_binding(packet: Packet, admission: Any) -> dict[str, str] | None:
    path = "input/revision-admission.json"
    if not isinstance(admission, dict):
        packet.input_unavailable = True
        packet.issue("invalid_revision_admission", path)
        return None
    if admission.get("schema") != "ub-review.revision_admission.v1":
        packet.input_unavailable = True
        packet.issue("unsupported_revision_admission", path)
        return None
    digest = admission.get("identity_digest")
    semantics = admission.get("semantics")
    reviewed = admission.get("reviewed_commit_oid")
    pr_head = admission.get("pr_head_commit")
    if not isinstance(digest, str) or SHA256_RE.fullmatch(digest) is None:
        packet.input_unavailable = True
        packet.issue("invalid_revision_digest", path)
    if semantics not in {"candidate_head", "merge_result"}:
        packet.input_unavailable = True
        packet.issue("invalid_review_semantics", path)
    if not valid_oid(reviewed):
        packet.input_unavailable = True
        packet.issue("invalid_reviewed_commit", path)
    if not valid_oid(pr_head):
        packet.input_unavailable = True
        packet.issue("invalid_pr_head_commit", path)
    if packet.input_unavailable:
        return None
    if semantics == "candidate_head" and reviewed != pr_head:
        packet.issue("candidate_head_identity_mismatch", path)
    return {
        "digest": digest,
        "semantics": semantics,
        "reviewed_commit": reviewed,
        "pr_head_commit": pr_head,
    }


def gate_projection(packet: Packet, gate: Any, binding: dict[str, str] | None) -> str | None:
    path = "review/gate_outcome.json"
    if not isinstance(gate, dict):
        packet.input_unavailable = True
        packet.issue("invalid_gate_projection", path)
        return None
    if gate.get("schema") != "ub-review.gate_outcome.v1":
        packet.input_unavailable = True
        packet.issue("unsupported_gate_projection", path)
    expected_revision = None
    if binding is not None:
        expected_revision = {
            "digest": binding["digest"],
            "semantics": binding["semantics"],
            "reviewed_commit": binding["reviewed_commit"],
        }
    if expected_revision is not None and gate.get("revision") != expected_revision:
        packet.issue("gate_revision_mismatch", path)
    publication = gate.get("publication_result")
    if publication not in {"posted", "not_needed", "failed", "not_proven"}:
        packet.input_unavailable = True
        packet.issue("invalid_gate_publication_result", path)
        return None
    return publication


def response_commit(response: Any) -> str | None:
    if not isinstance(response, dict):
        return None
    commit = response.get("commit_id")
    return commit if valid_oid(commit) else None


def post_result_state(
    packet: Packet,
    receipt: Any,
    binding: dict[str, str] | None,
) -> str:
    path = "review/post-result.json"
    if not isinstance(receipt, dict):
        packet.input_unavailable = True
        packet.issue("invalid_post_result", path)
        return "unverifiable"
    if receipt.get("schema_version") != 1 or receipt.get("status") != "ok":
        packet.input_unavailable = True
        packet.issue("invalid_post_result", path)
    for key in (
        "repo_valid",
        "review_json_exists",
        "review_json_valid",
        "token_present",
        "payload_written",
    ):
        if receipt.get(key) is not True:
            packet.issue("post_success_precondition_missing", path, key)
    status = receipt.get("http_status")
    if type(status) is not int or not 200 <= status < 300:
        packet.issue("post_success_http_status_invalid", path)
    if receipt.get("post_stdout_written") is True and not packet.path(
        "review/post-stdout.json"
    ).is_file():
        packet.issue("post_stdout_missing", "review/post-stdout.json")
    commit = response_commit(receipt.get("response"))
    if commit is None:
        packet.observe("post_response_head_unavailable", path)
        return "unverifiable"
    if binding is None:
        return "unverifiable"
    if commit != binding["pr_head_commit"]:
        packet.issue("post_response_head_mismatch", path, commit)
        return "failed"
    return "confirmed"


def post_error_state(packet: Packet, receipt: Any) -> str:
    path = "review/post-error.json"
    if not isinstance(receipt, dict):
        packet.input_unavailable = True
        packet.issue("invalid_post_error", path)
        return "unverifiable"
    if receipt.get("schema_version") != 1 or receipt.get("status") != "failed":
        packet.input_unavailable = True
        packet.issue("invalid_post_error", path)
    if not isinstance(receipt.get("error_kind"), str) or not receipt.get("error_kind"):
        packet.input_unavailable = True
        packet.issue("invalid_post_error_kind", path)
    if not isinstance(receipt.get("failure_stage"), str) or not receipt.get("failure_stage"):
        packet.input_unavailable = True
        packet.issue("invalid_post_failure_stage", path)
    return "failed"


def reconcile(root: Path) -> dict[str, Any]:
    packet = Packet(root)
    admission = packet.load("input/revision-admission.json", required=True)
    binding = revision_binding(packet, admission)
    gate = packet.load("review/gate_outcome.json", required=True)
    publication = gate_projection(packet, gate, binding)
    terminal = packet.load("review/terminal_state.json", required=True)
    terminal_valid = (
        isinstance(terminal, dict)
        and terminal.get("schema") == "ub-review.terminal_state.v1"
    )
    if not terminal_valid:
        packet.input_unavailable = True
        packet.issue("invalid_terminal_state", "review/terminal_state.json")

    review = packet.load("review/github-review.json")
    skip = packet.load("review/github-review-skip.json")
    result = packet.load("review/post-result.json")
    error = packet.load("review/post-error.json")

    review_present = review is not None
    skip_present = skip is not None
    result_present = result is not None
    error_present = error is not None

    if review_present == skip_present:
        packet.issue("prepared_review_xor_violation", "review")
        if not review_present:
            packet.input_unavailable = True
    if result_present and error_present:
        packet.issue("post_receipt_xor_violation", "review")

    preparation_state = "unverifiable"
    delivery_state = "unverifiable"

    if review_present and not skip_present:
        preparation_state = "prepared"
        if not isinstance(review, dict):
            packet.input_unavailable = True
            packet.issue("invalid_github_review", "review/github-review.json")
        elif not isinstance(review.get("body"), str) or not isinstance(
            review.get("comments"), list
        ):
            packet.input_unavailable = True
            packet.issue("invalid_github_review", "review/github-review.json")
        if result_present and not error_present:
            delivery_state = post_result_state(packet, result, binding)
        elif error_present and not result_present:
            delivery_state = post_error_state(packet, error)
        elif not result_present and not error_present:
            delivery_state = "prepared"
            packet.observe("post_attempt_not_recorded", "review/github-review.json")
    elif skip_present and not review_present:
        preparation_state = "not_needed"
        valid_skip = (
            isinstance(skip, dict)
            and skip.get("schema_version") == 1
            and skip.get("status") == "skipped"
            and skip.get("review_payload_status") in SKIP_STATUSES
            and skip.get("github_review_json") is None
        )
        if not valid_skip:
            packet.input_unavailable = True
            packet.issue("invalid_github_review_skip", "review/github-review-skip.json")
        delivery_state = "not_needed"
        if result_present and not error_present:
            if not (
                isinstance(result, dict)
                and result.get("schema_version") == 1
                and result.get("status") == "skipped"
            ):
                packet.issue("invalid_skip_post_result", "review/post-result.json")
        elif error_present and not result_present:
            packet.issue("post_error_present_for_skip", "review/post-error.json")

    if publication is not None:
        if delivery_state == "confirmed" and publication != "posted":
            packet.issue("confirmed_delivery_not_projected_posted", "review/gate_outcome.json")
        elif delivery_state == "failed" and publication == "posted":
            packet.issue("failed_delivery_projected_posted", "review/gate_outcome.json")
        elif delivery_state == "prepared" and publication == "posted":
            packet.issue("prepared_payload_projected_posted", "review/gate_outcome.json")
        elif delivery_state == "not_needed" and publication == "posted":
            packet.issue("skipped_review_projected_posted", "review/gate_outcome.json")

    complete = (
        binding is not None
        and terminal_valid
        and publication is not None
        and preparation_state != "unverifiable"
        and delivery_state != "unverifiable"
    )
    if packet.input_unavailable:
        status = "unverifiable"
    elif packet.issues or packet.issues_truncated:
        status = "contradictory"
    elif complete:
        status = "coherent"
    else:
        status = "unverifiable"

    report = {
        "schema": SCHEMA,
        "authority": "shadow-only",
        "status": status,
        "revision": binding,
        "preparation_state": preparation_state,
        "delivery_state": delivery_state,
        "gate_publication_result": publication,
        "input_unavailable": packet.input_unavailable,
        "issue_count_retained": len(packet.issues),
        "issues_truncated": packet.issues_truncated,
        "observations_truncated": packet.observations_truncated,
        "issues": [
            {"code": code, "artifact": artifact, "identity": identity}
            for code, artifact, identity in sorted(packet.issues)
        ],
        "observations": [
            {"code": code, "artifact": artifact, "identity": identity}
            for code, artifact, identity in sorted(packet.observations)
        ],
        "source_artifacts": [packet.sources[name] for name in sorted(packet.sources)],
    }
    if len(canonical(report)) > MAX_REPORT_BYTES:
        raise PacketError("publication reconciliation report exceeds hard byte limit")
    return report


def publish_report(root: Path, report: dict[str, Any]) -> None:
    packet = Packet(root)
    destination = packet.path(REPORT_PATH)
    destination.parent.mkdir(parents=True, exist_ok=True)
    data = canonical(report)
    if len(data) > MAX_REPORT_BYTES:
        raise PacketError("report byte budget exceeded")
    descriptor, temporary = tempfile.mkstemp(
        prefix=".publication-boundary-", dir=destination.parent
    )
    try:
        with os.fdopen(descriptor, "wb") as stream:
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
    parser.add_argument(
        "--write-report",
        action="store_true",
        help=f"atomically write only {REPORT_PATH}",
    )
    args = parser.parse_args(argv)
    try:
        if args.write_report:
            old_report = Packet(args.packet).path(REPORT_PATH)
            if old_report.exists():
                old_report.unlink()
        report = reconcile(args.packet)
        if args.write_report:
            publish_report(args.packet, report)
        sys.stdout.buffer.write(canonical(report))
        if report["input_unavailable"]:
            return 2
        return 0 if report["status"] == "coherent" else 1
    except (OSError, ValueError, KeyError, TypeError, AttributeError, RecursionError):
        print(
            "publication-boundary verification failed: input, output, or budget unavailable",
            file=sys.stderr,
        )
        return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
