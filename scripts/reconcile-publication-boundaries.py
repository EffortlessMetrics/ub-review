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
REVISION_ADMISSION_SCHEMA = "ub-review.revision_admission.v1"
REVISION_CANONICAL_VERSION = "ub-review.revision-identity.v1"
REVISION_DIGEST_DOMAIN = b"ub-review.revision-identity.digest.v1"
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
TERMINAL_STATUSES = {
    "needs-reviewer-attention",
    "sufficient",
    "artifact-only",
    "failed-to-review",
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

    def unavailable(self, code: str, artifact: str, identity: str = "") -> None:
        self.input_unavailable = True
        self.issue(code, artifact, identity)

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
                self.unavailable("missing_publication_artifact", name)
            return data
        if len(self.raw) >= MAX_INPUT_FILES:
            raise PacketError("input file budget exceeded")
        path = self.path(name)
        if not path.exists():
            self.raw[name] = None
            if required:
                self.unavailable("missing_publication_artifact", name)
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
            self.unavailable("invalid_publication_artifact", name)
            return None


def valid_oid(value: Any) -> bool:
    return (
        isinstance(value, str)
        and OID_RE.fullmatch(value) is not None
        and any(character != "0" for character in value)
    )


def parse_pair(value: str) -> tuple[str, str]:
    parts = value.split(" ")
    if len(parts) != 2 or not all(valid_oid(part) for part in parts):
        raise ValueError("revision pair must be exactly two valid object ids")
    return parts[0], parts[1]


def parse_revision_canonical(text: Any) -> dict[str, Any]:
    if not isinstance(text, str) or "\r" in text or not text.endswith("\n"):
        raise ValueError("canonical revision identity must be normalized LF text")
    lines = text[:-1].split("\n")
    if len(lines) != 8 or lines[0] != REVISION_CANONICAL_VERSION:
        raise ValueError("unsupported or malformed canonical revision identity")
    expected_keys = [
        "semantics",
        "base",
        "head",
        "reviewed",
        "merge",
        "changed_paths",
        "diff",
    ]
    values: dict[str, str] = {}
    for line, expected in zip(lines[1:], expected_keys, strict=True):
        key, separator, value = line.partition("=")
        if separator != "=" or key != expected or not value:
            raise ValueError("canonical revision field order or value is invalid")
        values[key] = value

    semantics = values["semantics"]
    if semantics not in {"candidate_head", "merge_result"}:
        raise ValueError("unknown canonical review semantics")
    base = parse_pair(values["base"])
    head = parse_pair(values["head"])
    reviewed = parse_pair(values["reviewed"])
    merge = None if values["merge"] == "-" else parse_pair(values["merge"])
    if SHA256_RE.fullmatch(values["changed_paths"]) is None:
        raise ValueError("invalid changed-path digest")
    if SHA256_RE.fullmatch(values["diff"]) is None:
        raise ValueError("invalid diff digest")

    pairs = [base, head, reviewed] + ([] if merge is None else [merge])
    widths = {len(oid) for pair in pairs for oid in pair}
    if len(widths) != 1:
        raise ValueError("mixed object-id widths")
    if semantics == "candidate_head":
        if merge is not None or reviewed != head:
            raise ValueError("candidate-head canonical identity is contradictory")
    elif merge is None or reviewed != merge:
        raise ValueError("merge-result canonical identity is contradictory")

    normalized_lines = [
        REVISION_CANONICAL_VERSION,
        f"semantics={semantics}",
        f"base={base[0]} {base[1]}",
        f"head={head[0]} {head[1]}",
        f"reviewed={reviewed[0]} {reviewed[1]}",
        "merge=-" if merge is None else f"merge={merge[0]} {merge[1]}",
        f"changed_paths={values['changed_paths']}",
        f"diff={values['diff']}",
    ]
    normalized = "\n".join(normalized_lines) + "\n"
    if normalized != text:
        raise ValueError("canonical revision identity is not normalized")
    digest = hashlib.sha256(
        REVISION_DIGEST_DOMAIN + b"\x00" + normalized.encode("utf-8")
    ).hexdigest()
    return {
        "canonical": normalized,
        "digest": digest,
        "semantics": semantics,
        "head_commit": head[0],
        "reviewed_commit": reviewed[0],
    }


def revision_binding(packet: Packet, admission: Any) -> dict[str, str] | None:
    path = "input/revision-admission.json"
    if not isinstance(admission, dict):
        packet.unavailable("invalid_revision_admission", path)
        return None
    if admission.get("schema") != REVISION_ADMISSION_SCHEMA:
        packet.unavailable("unsupported_revision_admission", path)
        return None
    try:
        parsed = parse_revision_canonical(admission.get("identity_canonical"))
    except ValueError:
        packet.unavailable("invalid_revision_canonical", path)
        return None

    valid = True
    digest = admission.get("identity_digest")
    semantics = admission.get("semantics")
    reviewed = admission.get("reviewed_commit_oid")
    pr_head = admission.get("pr_head_commit")
    if digest != parsed["digest"]:
        packet.unavailable("revision_digest_mismatch", path)
        valid = False
    if semantics != parsed["semantics"]:
        packet.unavailable("revision_semantics_mismatch", path)
        valid = False
    if reviewed != parsed["reviewed_commit"]:
        packet.unavailable("reviewed_commit_mismatch", path)
        valid = False
    if not valid_oid(pr_head) or pr_head != parsed["head_commit"]:
        packet.unavailable("pr_head_commit_mismatch", path)
        valid = False
    if not valid:
        return None
    return {
        "digest": parsed["digest"],
        "semantics": parsed["semantics"],
        "reviewed_commit": parsed["reviewed_commit"],
        "pr_head_commit": parsed["head_commit"],
    }


def gate_projection(packet: Packet, gate: Any, binding: dict[str, str] | None) -> str | None:
    path = "review/gate_outcome.json"
    if not isinstance(gate, dict):
        packet.unavailable("invalid_gate_projection", path)
        return None
    if gate.get("schema") != "ub-review.gate_outcome.v1":
        packet.unavailable("unsupported_gate_projection", path)
        return None
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
        packet.unavailable("invalid_gate_publication_result", path)
        return None
    return publication


def terminal_projection(packet: Packet, terminal: Any) -> dict[str, Any] | None:
    path = "review/terminal_state.json"
    if not isinstance(terminal, dict) or terminal.get("schema") != "ub-review.terminal_state.v1":
        packet.unavailable("invalid_terminal_state", path)
        return None
    status = terminal.get("status")
    payload = terminal.get("review_payload_status")
    reviewer_value = terminal.get("reviewer_value_present")
    valid = True
    if status not in TERMINAL_STATUSES:
        packet.unavailable("invalid_terminal_status", path)
        valid = False
    if payload not in {"prepared", *SKIP_STATUSES}:
        packet.unavailable("invalid_terminal_payload_status", path)
        valid = False
    if type(reviewer_value) is not bool:
        packet.unavailable("invalid_terminal_reviewer_value", path)
        valid = False
    if not valid:
        return None
    return {
        "status": status,
        "review_payload_status": payload,
        "reviewer_value_present": reviewer_value,
    }


def prepared_review_facts(review: Any) -> dict[str, Any] | None:
    if not (
        isinstance(review, dict)
        and review.get("event") == "COMMENT"
        and isinstance(review.get("body"), str)
        and isinstance(review.get("comments"), list)
    ):
        return None
    return {
        "event": "COMMENT",
        "body_bytes": len(review["body"].encode("utf-8")),
        "comment_count": len(review["comments"]),
    }


def valid_skip_receipt(skip: Any) -> bool:
    return (
        isinstance(skip, dict)
        and skip.get("schema_version") == 1
        and skip.get("status") == "skipped"
        and skip.get("review_payload_status") in SKIP_STATUSES
        and skip.get("terminal_state") in TERMINAL_STATUSES
        and skip.get("github_review_json") is None
    )


def response_commit(response: Any) -> str | None:
    if not isinstance(response, dict):
        return None
    commit = response.get("commit_id")
    return commit if valid_oid(commit) else None


def post_result_state(
    packet: Packet,
    receipt: Any,
    binding: dict[str, str] | None,
    prepared: dict[str, Any],
) -> str:
    path = "review/post-result.json"
    if not isinstance(receipt, dict):
        packet.unavailable("invalid_post_result", path)
        return "unverifiable"
    valid = True
    if receipt.get("schema_version") != 1 or receipt.get("status") != "ok":
        packet.unavailable("invalid_post_result", path)
        valid = False
    for key in (
        "repo_valid",
        "review_json_exists",
        "review_json_valid",
        "token_present",
        "payload_written",
        "post_stdout_written",
        "post_stderr_written",
    ):
        if receipt.get(key) is not True:
            packet.unavailable("post_success_precondition_missing", path, key)
            valid = False
    expected_metadata = {
        "review_event": prepared["event"],
        "review_body_bytes": prepared["body_bytes"],
        "review_comment_count": prepared["comment_count"],
        "comments": prepared["comment_count"],
    }
    for key, expected in expected_metadata.items():
        if receipt.get(key) != expected:
            packet.unavailable("post_success_payload_mismatch", path, key)
            valid = False
    pull_number = receipt.get("pull_number")
    if type(pull_number) is not int or pull_number <= 0:
        packet.unavailable("invalid_post_pull_number", path)
        valid = False
    status = receipt.get("http_status")
    if type(status) is not int or not 200 <= status < 300:
        packet.unavailable("post_success_http_status_invalid", path)
        valid = False
    for artifact, code in (
        ("review/post-stdout.json", "post_stdout_missing"),
        ("review/post-stderr.txt", "post_stderr_missing"),
    ):
        try:
            exists = packet.path(artifact).is_file()
        except OSError:
            exists = False
        if not exists:
            packet.unavailable(code, artifact)
            valid = False
    response = receipt.get("response")
    if not isinstance(response, dict) or response.get("state") != "COMMENTED":
        packet.unavailable("invalid_post_response_state", path)
        valid = False
    commit = response_commit(response)
    if commit is None:
        packet.observe("post_response_head_unavailable", path)
        valid = False
    if not valid or binding is None:
        return "unverifiable"
    if commit != binding["pr_head_commit"]:
        packet.issue("post_response_head_mismatch", path, commit)
        return "failed"
    return "confirmed"


def post_error_state(packet: Packet, receipt: Any) -> str:
    path = "review/post-error.json"
    if not isinstance(receipt, dict):
        packet.unavailable("invalid_post_error", path)
        return "unverifiable"
    valid = True
    if receipt.get("schema_version") != 1 or receipt.get("status") != "failed":
        packet.unavailable("invalid_post_error", path)
        valid = False
    if not isinstance(receipt.get("error_kind"), str) or not receipt.get("error_kind"):
        packet.unavailable("invalid_post_error_kind", path)
        valid = False
    if not isinstance(receipt.get("failure_stage"), str) or not receipt.get("failure_stage"):
        packet.unavailable("invalid_post_failure_stage", path)
        valid = False
    return "failed" if valid else "unverifiable"


def valid_skip_post_result(receipt: Any) -> bool:
    return (
        isinstance(receipt, dict)
        and receipt.get("schema_version") == 1
        and receipt.get("status") == "skipped"
    )


def reconcile(root: Path) -> dict[str, Any]:
    packet = Packet(root)
    admission = packet.load("input/revision-admission.json", required=True)
    binding = revision_binding(packet, admission)
    gate = packet.load("review/gate_outcome.json", required=True)
    publication = gate_projection(packet, gate, binding)
    terminal_value = packet.load("review/terminal_state.json", required=True)
    terminal = terminal_projection(packet, terminal_value)

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
        prepared = prepared_review_facts(review)
        if prepared is not None:
            preparation_state = "prepared"
            if terminal is not None and terminal["review_payload_status"] != "prepared":
                packet.issue("terminal_payload_mismatch", "review/terminal_state.json")
            if result_present and not error_present:
                delivery_state = post_result_state(packet, result, binding, prepared)
            elif error_present and not result_present:
                delivery_state = post_error_state(packet, error)
            elif not result_present and not error_present:
                delivery_state = "prepared"
                packet.observe("post_attempt_not_recorded", "review/github-review.json")
        else:
            packet.unavailable("invalid_github_review", "review/github-review.json")
    elif skip_present and not review_present:
        if valid_skip_receipt(skip):
            preparation_state = "not_needed"
            if terminal is not None:
                if terminal["review_payload_status"] != skip["review_payload_status"]:
                    packet.issue("terminal_payload_mismatch", "review/terminal_state.json")
                if terminal["status"] != skip["terminal_state"]:
                    packet.issue("terminal_status_mismatch", "review/terminal_state.json")
            if result_present and not error_present:
                if valid_skip_post_result(result):
                    delivery_state = "not_needed"
                else:
                    packet.issue("invalid_skip_post_result", "review/post-result.json")
            elif error_present and not result_present:
                delivery_state = post_error_state(packet, error)
                if delivery_state == "failed":
                    packet.issue("post_error_present_for_skip", "review/post-error.json")
            elif not result_present and not error_present:
                delivery_state = "not_needed"
        else:
            packet.unavailable("invalid_github_review_skip", "review/github-review-skip.json")

    if publication is not None:
        if delivery_state == "confirmed" and publication != "posted":
            packet.issue("confirmed_delivery_not_projected_posted", "review/gate_outcome.json")
        elif delivery_state == "failed" and publication == "posted":
            packet.issue("failed_delivery_projected_posted", "review/gate_outcome.json")
        elif delivery_state == "prepared" and publication == "posted":
            packet.issue("prepared_payload_projected_posted", "review/gate_outcome.json")
        elif delivery_state == "not_needed" and publication == "posted":
            packet.issue("skipped_review_projected_posted", "review/gate_outcome.json")
        elif delivery_state == "unverifiable" and publication == "posted":
            packet.issue("unverifiable_delivery_projected_posted", "review/gate_outcome.json")

    complete = (
        binding is not None
        and terminal is not None
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
