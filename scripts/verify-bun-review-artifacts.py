#!/usr/bin/env python3
"""Verify that a UB review packet satisfies the Bun v0 artifact contract."""

from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import re
import sys
from typing import Any


LANES = ["ub", "source-route", "tests", "arch", "opposition", "security"]
SENSORS = ["tokmd", "ripr", "unsafe-review", "ast-grep"]
APPROVAL_LINES = {
    "lgtm",
    "looks good",
    "clean",
    "solid",
    "no issues found",
    "no actionable findings",
    "no actionable",
}
SECRET_MARKERS = [
    "Authorization:",
    "Bearer ",
    "X-Api-Key:",
    "X-API-Key:",
    "github_token",
    "GITHUB_TOKEN",
    "UB_REVIEW_GITHUB_TOKEN",
    "UB_REVIEW_MINIMAX_API_KEY",
    "UB_REVIEW_OPENCODE_API_KEY",
]


def fail(message: str) -> None:
    print(f"verify-bun-review-artifacts: {message}", file=sys.stderr)
    raise SystemExit(1)


def read_text(path: pathlib.Path) -> str:
    try:
        return path.read_text(encoding="utf-8")
    except FileNotFoundError:
        fail(f"missing {path}")
    except UnicodeDecodeError as error:
        fail(f"invalid UTF-8 in {path}: {error}")


def load_json(path: pathlib.Path) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        fail(f"missing {path}")
    except json.JSONDecodeError as error:
        fail(f"invalid JSON in {path}: {error}")


def require_file(path: pathlib.Path) -> pathlib.Path:
    if not path.is_file():
        fail(f"missing file {path}")
    return path


def no_standalone_approval_line(text: str, path: pathlib.Path) -> None:
    for line_number, line in enumerate(text.splitlines(), start=1):
        normalized = (
            line.strip()
            .removeprefix("- ")
            .removeprefix("* ")
            .strip()
            .lower()
        )
        if normalized in APPROVAL_LINES:
            fail(f"standalone approval line in {path}:{line_number}: {line!r}")


def require_common_tree(root: pathlib.Path) -> None:
    for path in [
        "input/changed-files.txt",
        "input/diff.patch",
        "input/diff-context.json",
        "events.ndjson",
        "running-summary.md",
        "review/shared_context.md",
        "review/provider-preflight-status.json",
        "review/metrics.json",
        "review/review.json",
        "review/review.md",
        "review/observations.json",
        "review/unique_observations.json",
        "review/merged_observations.json",
        "review/dropped_observations.json",
        "review/proof_requests.json",
        "review/proof_plan.md",
        "proof_requests.ndjson",
    ]:
        require_file(root / path)
    if not (root / "review/github-review.json").exists() and not (
        root / "review/github-review-skip.json"
    ).exists():
        fail("neither github-review.json nor github-review-skip.json exists")
    if (root / "review/github-review.json").exists() and (
        root / "review/github-review-skip.json"
    ).exists():
        fail("both github-review.json and github-review-skip.json exist")

    for sensor in SENSORS:
        require_file(root / "sensors" / sensor / "ub-review-sensor-status.json")

    for lane in LANES:
        lane_path = require_file(root / "lanes" / f"{lane}.md")
        lane_text = read_text(lane_path)
        if f"[{lane}]" not in lane_text:
            fail(f"lane packet {lane_path} does not include [{lane}] prefix")
        no_standalone_approval_line(lane_text, lane_path)


def require_summary(root: pathlib.Path) -> None:
    summary_path = root / "running-summary.md"
    summary = read_text(summary_path)
    for heading in [
        "## Missing evidence",
        "## Provider preflights",
        "## Model lane status",
        "## Lane packets",
    ]:
        if heading not in summary:
            fail(f"{summary_path} missing {heading}")
    no_standalone_approval_line(summary, summary_path)


def require_review(root: pathlib.Path, max_inline_comments: int | None) -> dict:
    review = load_json(root / "review/review.json")
    review_body = read_text(root / "review/review.md")
    shared_context = read_text(root / "review/shared_context.md")

    shared_context_id = review.get("shared_context_id")
    if not isinstance(shared_context_id, str) or not re.fullmatch(
        r"[0-9a-f]{64}", shared_context_id
    ):
        fail("review.json shared_context_id is not a 64-character hex digest")
    if review.get("mode") != "review-direct":
        fail(f"review.json mode expected review-direct, got {review.get('mode')!r}")
    if review.get("posting") not in {"review", "artifact-only"}:
        fail(f"review.json posting has unexpected value {review.get('posting')!r}")
    if not isinstance(review.get("model_lanes"), list):
        fail("review.json model_lanes is not an array")
    if "## UB Ledger Context" not in shared_context:
        fail("shared_context.md missing UB ledger context section")

    for heading in [
        "## Decision",
        "## Confirmed findings",
        "## Summary-only findings",
        "## Failed objections",
        "## Residual risk",
        "## Parked follow-ups",
        "## Missing or failed evidence",
    ]:
        if heading not in review_body:
            fail(f"review.md missing {heading}")
    no_standalone_approval_line(review_body, root / "review/review.md")

    github_review_path = root / "review/github-review.json"
    github_skip_path = root / "review/github-review-skip.json"
    if github_review_path.exists():
        github_review = load_json(github_review_path)
        if github_review.get("event") != "COMMENT":
            fail(f"github-review.json event expected COMMENT, got {github_review.get('event')!r}")
        body = github_review.get("body")
        if not isinstance(body, str) or "## Decision" not in body:
            fail("github-review.json body is missing the review summary")
        no_standalone_approval_line(body, github_review_path)
        comments = github_review.get("comments")
        if not isinstance(comments, list):
            fail("github-review.json comments is not an array")
        if max_inline_comments is None:
            max_inline_comments = int(review.get("max_inline_comments", 8))
        if len(comments) > max_inline_comments:
            fail(
                f"github-review.json has {len(comments)} comments, "
                f"over max {max_inline_comments}"
            )
        for index, comment in enumerate(comments):
            require_github_comment(comment, index)
    else:
        skip = load_json(github_skip_path)
        if skip.get("status") != "skipped":
            fail(f"github-review-skip.json status expected skipped, got {skip.get('status')!r}")
        if skip.get("review_payload_status") != "skipped_empty_smoke":
            fail(
                "github-review-skip.json review_payload_status expected skipped_empty_smoke"
            )

    return review


def require_github_comment(comment: dict, index: int) -> None:
    path = comment.get("path")
    if not isinstance(path, str) or not path or path.startswith(("/", "\\")) or ".." in pathlib.PurePosixPath(path).parts:
        fail(f"github review comment {index} path is not repo-relative: {path!r}")
    if comment.get("side") != "RIGHT":
        fail(f"github review comment {index} side expected RIGHT, got {comment.get('side')!r}")
    line = comment.get("line")
    if not isinstance(line, int) or line <= 0:
        fail(f"github review comment {index} line is invalid: {line!r}")
    body = comment.get("body")
    if not isinstance(body, str) or not re.match(r"^\[[a-z0-9-]+\]", body):
        fail(f"github review comment {index} body lacks lane prefix")
    if len(body) > 1_200:
        fail(f"github review comment {index} body exceeds 1200 characters")
    no_standalone_approval_line(body, pathlib.Path("review/github-review.json"))


def require_metrics(root: pathlib.Path, review: dict) -> dict:
    metrics = load_json(root / "review/metrics.json")
    if metrics.get("schema_version") != 1:
        fail(f"metrics schema_version expected 1, got {metrics.get('schema_version')!r}")
    if metrics.get("shared_context_id") != review.get("shared_context_id"):
        fail("metrics shared_context_id does not match review.json")
    if metrics.get("mode") != review.get("mode"):
        fail("metrics mode does not match review.json")
    if metrics.get("provider_policy") != review.get("provider_policy"):
        fail("metrics provider_policy does not match review.json")
    if metrics.get("inline_comments") != len(review.get("inline_comments", [])):
        fail("metrics inline_comments does not match review.json")
    if metrics.get("summary_only_findings") != len(review.get("summary_only_findings", [])):
        fail("metrics summary_only_findings does not match review.json")
    if not isinstance(metrics.get("observations"), int):
        fail("metrics observations is not an integer")
    if not isinstance(metrics.get("proof_requests"), int):
        fail("metrics proof_requests is not an integer")
    observations = load_json(root / "review/observations.json")
    if not isinstance(observations, list):
        fail("review/observations.json is not an array")
    if metrics.get("observations") != len(observations):
        fail("metrics observations does not match review/observations.json")
    require_observation_summary_artifacts(root, observations)
    proof_requests = load_json(root / "review/proof_requests.json")
    if not isinstance(proof_requests, list):
        fail("review/proof_requests.json is not an array")
    if metrics.get("proof_requests") != len(proof_requests):
        fail("metrics proof_requests does not match review/proof_requests.json")
    if review.get("proof_requests", []) != proof_requests:
        fail("review proof_requests does not match review/proof_requests.json")
    require_proof_request_ndjson(root, proof_requests)
    require_observation_files(root, observations)
    if (root / "review/github-review-skip.json").exists():
        if metrics.get("review_payload_status") != "skipped_empty_smoke":
            fail("metrics review_payload_status does not match github-review-skip.json")
        if metrics.get("github_review_body_bytes") != 0:
            fail("metrics github_review_body_bytes must be 0 for skipped review payloads")
        if metrics.get("github_review_comments") != 0:
            fail("metrics github_review_comments must be 0 for skipped review payloads")
    models = metrics.get("models")
    if not isinstance(models, dict):
        fail("metrics.models is missing")
    return metrics


def require_proof_request_ndjson(root: pathlib.Path, proof_requests: list[dict]) -> None:
    for request in proof_requests:
        require_proof_request_schema(request)
    ndjson_path = root / "proof_requests.ndjson"
    text = read_text(ndjson_path)
    lines = [line for line in text.splitlines() if line.strip()]
    if len(lines) != len(proof_requests):
        fail("proof_requests.ndjson line count does not match review/proof_requests.json")
    for index, line in enumerate(lines):
        try:
            parsed = json.loads(line)
        except json.JSONDecodeError as error:
            fail(f"invalid proof_requests.ndjson line {index + 1}: {error}")
        if parsed != proof_requests[index]:
            fail(f"proof_requests.ndjson line {index + 1} does not match JSON artifact")
    proof_plan = read_text(root / "review/proof_plan.md")
    if "# Proof request plan" not in proof_plan:
        fail("review/proof_plan.md missing heading")
    if proof_requests and "Proof requests are passive" not in proof_plan:
        fail("review/proof_plan.md does not mark proof requests as passive")
    if not proof_requests and "No proof requests were emitted" not in proof_plan:
        fail("review/proof_plan.md missing empty proof request note")


def require_observation_files(root: pathlib.Path, observations: list[dict]) -> None:
    observation_dir = root / "observations"
    if not observation_dir.is_dir():
        fail("missing observations directory")
    for observation in observations:
        require_observation_schema(observation)
    lanes = {observation["lane"] for observation in observations}
    for lane in lanes:
        path = observation_dir / f"{sanitize_artifact_name(lane)}.ndjson"
        require_file(path)
        lane_observations = []
        for line_number, line in enumerate(read_text(path).splitlines(), start=1):
            if not line.strip():
                continue
            try:
                parsed = json.loads(line)
            except json.JSONDecodeError as error:
                fail(f"invalid observation NDJSON {path}:{line_number}: {error}")
            if parsed.get("lane") != lane:
                fail(f"observation NDJSON {path}:{line_number} has wrong lane")
            require_observation_schema(parsed)
            lane_observations.append(parsed)
        expected = [observation for observation in observations if observation["lane"] == lane]
        if lane_observations != expected:
            fail(f"observation NDJSON {path} does not match review/observations.json")


def require_observation_summary_artifacts(
    root: pathlib.Path, observations: list[dict]
) -> None:
    unique = load_json(root / "review/unique_observations.json")
    merged = load_json(root / "review/merged_observations.json")
    dropped = load_json(root / "review/dropped_observations.json")
    if not isinstance(unique, list):
        fail("review/unique_observations.json is not an array")
    if not isinstance(merged, list):
        fail("review/merged_observations.json is not an array")
    if not isinstance(dropped, list):
        fail("review/dropped_observations.json is not an array")

    expected_unique, expected_merged, expected_dropped = expected_observation_groups(
        observations
    )
    if unique != expected_unique:
        fail("unique_observations.json does not match raw observation grouping")
    if merged != expected_merged:
        fail("merged_observations.json does not match raw observation grouping")
    if dropped != expected_dropped:
        fail("dropped_observations.json does not match raw observation grouping")

    observation_ids = {
        observation["id"]
        for observation in observations
        if isinstance(observation.get("id"), str)
    }
    unique_observation_ids: set[str] = set()
    group_ids: set[str] = set()
    duplicate_count = 0
    for group in unique:
        require_observation_group_schema(group)
        group_ids.add(group["id"])
        ids = group["observation_ids"]
        for observation_id in ids:
            if observation_id not in observation_ids:
                fail(f"unique observation group references unknown observation: {group!r}")
        unique_observation_ids.update(ids)
        if group["duplicate_count"] != max(0, len(ids) - 1):
            fail(f"unique observation group duplicate_count is wrong: {group!r}")
        duplicate_count += group["duplicate_count"]
    if unique_observation_ids != observation_ids:
        fail("unique_observations.json does not cover the raw observation ids exactly")

    dropped_ids = set()
    for record in merged:
        require_merged_observation_schema(record, group_ids, observation_ids)
    for record in dropped:
        require_dropped_observation_schema(record, group_ids, observation_ids)
        if record["observation_id"] in dropped_ids:
            fail(f"dropped observation is listed more than once: {record!r}")
        dropped_ids.add(record["observation_id"])
    if len(dropped) != duplicate_count:
        fail("dropped_observations.json count does not match grouped duplicate count")


def expected_observation_groups(
    observations: list[dict],
) -> tuple[list[dict], list[dict], list[dict]]:
    groups: list[dict] = []
    indexes: dict[str, int] = {}
    for observation in observations:
        key = observation.get("dedupe_key") or observation.get("fingerprint")
        if not isinstance(key, str) or not key:
            fail(f"observation cannot be grouped: {observation!r}")
        if key in indexes:
            merge_expected_observation_group(groups[indexes[key]], observation)
        else:
            group_id = expected_observation_group_id(len(groups), key)
            groups.append(
                {
                    "schema": "ub-review.observation_group.v1",
                    "id": group_id,
                    "dedupe_key": key,
                    "claim": observation["claim"],
                    "kind": observation["kind"],
                    "status": observation["status"],
                    "severity": observation["severity"],
                    "confidence": observation["confidence"],
                    "path": observation.get("path"),
                    "line": observation.get("line"),
                    "evidence": observation.get("evidence", [])[:3],
                    "lanes": [observation["lane"]],
                    "sources": [observation["source"]],
                    "observation_ids": [observation["id"]],
                    "duplicate_count": 0,
                }
            )
            indexes[key] = len(groups) - 1

    merged = []
    dropped = []
    by_id = {observation["id"]: observation for observation in observations}
    for group in groups:
        ids = group["observation_ids"]
        if len(ids) <= 1:
            continue
        duplicate_ids = ids[1:]
        merged.append(
            {
                "schema": "ub-review.merged_observation.v1",
                "group_id": group["id"],
                "dedupe_key": group["dedupe_key"],
                "kept_observation_id": ids[0],
                "merged_observation_ids": duplicate_ids,
                "lanes": group["lanes"],
                "reason": "merged_duplicate_dedupe_key",
            }
        )
        for observation_id in duplicate_ids:
            dropped.append(
                {
                    "schema": "ub-review.dropped_observation.v1",
                    "observation_id": observation_id,
                    "group_id": group["id"],
                    "dedupe_key": group["dedupe_key"],
                    "lane": by_id[observation_id]["lane"],
                    "reason": "merged_into_unique_observation",
                }
            )
    return groups, merged, dropped


def merge_expected_observation_group(group: dict, observation: dict) -> None:
    if severity_rank(observation["severity"]) > severity_rank(group["severity"]):
        group["severity"] = observation["severity"]
    if confidence_rank(observation["confidence"]) > confidence_rank(group["confidence"]):
        group["confidence"] = observation["confidence"]
    if observation_status_rank(observation["status"]) > observation_status_rank(group["status"]):
        group["status"] = observation["status"]
    if group.get("path") is None:
        group["path"] = observation.get("path")
    if group.get("line") is None:
        group["line"] = observation.get("line")
    if observation["lane"] not in group["lanes"]:
        group["lanes"].append(observation["lane"])
    if observation["source"] not in group["sources"]:
        group["sources"].append(observation["source"])
    group["observation_ids"].append(observation["id"])
    group["duplicate_count"] = max(0, len(group["observation_ids"]) - 1)
    for evidence in observation.get("evidence", []):
        if len(group["evidence"]) >= 3:
            break
        if evidence not in group["evidence"]:
            group["evidence"].append(evidence)


def expected_observation_group_id(index: int, dedupe_key: str) -> str:
    digest = hashlib.sha256(dedupe_key.encode("utf-8")).hexdigest()
    return f"obsgrp-{index:04d}-{digest[:12]}"


def severity_rank(value: str) -> int:
    return {
        "blocker": 4,
        "high": 3,
        "medium": 2,
        "low": 1,
    }.get(value, 0)


def confidence_rank(value: str) -> int:
    return {
        "high": 3,
        "medium-high": 2,
        "medium": 1,
        "low": 0,
    }.get(value, 0)


def observation_status_rank(value: str) -> int:
    return {
        "refuted": 7,
        "confirmed": 6,
        "parked": 5,
        "demoted": 4,
        "open": 3,
        "covered": 2,
        "duplicate": 1,
    }.get(value, 0)


def require_observation_group_schema(group: dict) -> None:
    if group.get("schema") != "ub-review.observation_group.v1":
        fail(f"observation group has wrong schema: {group!r}")
    for field in [
        "id",
        "dedupe_key",
        "claim",
        "kind",
        "status",
        "severity",
        "confidence",
    ]:
        if not isinstance(group.get(field), str) or not group[field]:
            fail(f"observation group missing string field {field}: {group!r}")
    for field in ["evidence", "lanes", "sources", "observation_ids"]:
        values = group.get(field)
        if not isinstance(values, list) or not all(
            isinstance(item, str) and item for item in values
        ):
            fail(f"observation group {field} is not a non-empty string array: {group!r}")
    if not isinstance(group.get("duplicate_count"), int) or group["duplicate_count"] < 0:
        fail(f"observation group duplicate_count is invalid: {group!r}")
    path = group.get("path")
    if path is not None and (not isinstance(path, str) or path.startswith(("/", "\\"))):
        fail(f"observation group path is not repo-relative: {group!r}")
    line = group.get("line")
    if line is not None and (not isinstance(line, int) or line <= 0):
        fail(f"observation group line is invalid: {group!r}")


def require_merged_observation_schema(
    record: dict, group_ids: set[str], observation_ids: set[str]
) -> None:
    if record.get("schema") != "ub-review.merged_observation.v1":
        fail(f"merged observation has wrong schema: {record!r}")
    for field in ["group_id", "dedupe_key", "kept_observation_id", "reason"]:
        if not isinstance(record.get(field), str) or not record[field]:
            fail(f"merged observation missing string field {field}: {record!r}")
    if record["group_id"] not in group_ids:
        fail(f"merged observation references unknown group: {record!r}")
    if record["kept_observation_id"] not in observation_ids:
        fail(f"merged observation references unknown kept observation: {record!r}")
    for field in ["merged_observation_ids", "lanes"]:
        values = record.get(field)
        if not isinstance(values, list) or not all(
            isinstance(item, str) and item for item in values
        ):
            fail(f"merged observation {field} is not a non-empty string array: {record!r}")
    for observation_id in record["merged_observation_ids"]:
        if observation_id not in observation_ids:
            fail(f"merged observation references unknown duplicate: {record!r}")


def require_dropped_observation_schema(
    record: dict, group_ids: set[str], observation_ids: set[str]
) -> None:
    if record.get("schema") != "ub-review.dropped_observation.v1":
        fail(f"dropped observation has wrong schema: {record!r}")
    for field in ["observation_id", "group_id", "dedupe_key", "lane", "reason"]:
        if not isinstance(record.get(field), str) or not record[field]:
            fail(f"dropped observation missing string field {field}: {record!r}")
    if record["group_id"] not in group_ids:
        fail(f"dropped observation references unknown group: {record!r}")
    if record["observation_id"] not in observation_ids:
        fail(f"dropped observation references unknown observation: {record!r}")


def require_observation_schema(observation: dict) -> None:
    if observation.get("schema") != "ub-review.observation.v1":
        fail(f"observation has wrong schema: {observation!r}")
    for field in [
        "id",
        "lane",
        "question",
        "claim",
        "kind",
        "status",
        "severity",
        "confidence",
        "fingerprint",
        "dedupe_key",
        "source",
    ]:
        if not isinstance(observation.get(field), str) or not observation[field]:
            fail(f"observation missing string field {field}: {observation!r}")
    if observation["kind"] not in {
        "bug",
        "verification-question",
        "missing-evidence",
        "test-gap",
        "source-route-gap",
        "security-risk",
        "false-premise",
        "parked-follow-up",
        "resolved-check",
    }:
        fail(f"observation has unsupported kind: {observation!r}")
    if observation["status"] not in {
        "open",
        "covered",
        "confirmed",
        "refuted",
        "demoted",
        "parked",
        "duplicate",
    }:
        fail(f"observation has unsupported status: {observation!r}")
    if observation["severity"] not in {"blocker", "high", "medium", "low"}:
        fail(f"observation has unsupported severity: {observation!r}")
    if observation["confidence"] not in {"high", "medium-high", "medium", "low"}:
        fail(f"observation has unsupported confidence: {observation!r}")
    if not re.fullmatch(r"[0-9a-f]{64}", observation["fingerprint"]):
        fail(f"observation fingerprint is not a SHA-256 hex digest: {observation!r}")
    evidence = observation.get("evidence")
    if not isinstance(evidence, list) or not all(isinstance(item, str) for item in evidence):
        fail(f"observation evidence is not a string array: {observation!r}")
    path = observation.get("path")
    if path is not None and (not isinstance(path, str) or path.startswith(("/", "\\"))):
        fail(f"observation path is not repo-relative: {observation!r}")
    line = observation.get("line")
    if line is not None and (not isinstance(line, int) or line <= 0):
        fail(f"observation line is invalid: {observation!r}")


def require_proof_request_schema(request: dict) -> None:
    if request.get("schema") != "ub-review.proof_request.v1":
        fail(f"proof request has wrong schema: {request!r}")
    for field in ["id", "lane", "command", "reason", "cost", "status"]:
        if not isinstance(request.get(field), str) or not request[field]:
            fail(f"proof request missing string field {field}: {request!r}")
    requested_by = request.get("requested_by")
    if not isinstance(requested_by, list) or not all(
        isinstance(item, str) and item for item in requested_by
    ):
        fail(f"proof request requested_by is not a non-empty string array: {request!r}")
    if request["lane"] not in requested_by:
        fail(f"proof request lane is not listed in requested_by: {request!r}")
    timeout = request.get("timeout_sec")
    if not isinstance(timeout, int) or timeout <= 0 or timeout > 900:
        fail(f"proof request timeout_sec is invalid: {request!r}")
    if not isinstance(request.get("required"), bool):
        fail(f"proof request required is not boolean: {request!r}")
    if request["status"] not in {"requested", "invalid"}:
        fail(f"proof request has unsupported status: {request!r}")


def sanitize_artifact_name(value: str) -> str:
    return "".join(ch if ch.isalnum() or ch in "-_" else "-" for ch in value)


def require_sensor_receipts(root: pathlib.Path) -> None:
    for sensor in SENSORS:
        receipt = load_json(root / "sensors" / sensor / "ub-review-sensor-status.json")
        if receipt.get("sensor") != sensor:
            fail(f"{sensor} receipt has wrong sensor id {receipt.get('sensor')!r}")
        if receipt.get("status") not in {"ok", "missing", "skipped", "failed", "timed_out"}:
            fail(f"{sensor} receipt has unsupported status {receipt.get('status')!r}")
        if "reason" not in receipt:
            fail(f"{sensor} receipt missing reason")


def require_model_receipts(review: dict, metrics: dict, min_ok_model_lanes: int) -> None:
    if min_ok_model_lanes <= 0:
        return
    preflights = review.get("provider_preflights")
    if not isinstance(preflights, list) or not preflights:
        fail("review.json provider_preflights is empty")
    ok_preflights = [receipt for receipt in preflights if receipt.get("status") == "ok"]
    if not ok_preflights:
        fail("no ok provider preflight receipt")
    ok_lanes = [
        lane
        for lane in review.get("model_lanes", [])
        if lane.get("status") == "ok" and lane.get("lane") != "refuter"
    ]
    if len(ok_lanes) < min_ok_model_lanes:
        fail(
            f"expected at least {min_ok_model_lanes} ok model lanes, got {len(ok_lanes)}"
        )
    model_metrics = metrics.get("models", {})
    if model_metrics.get("provider_preflight_calls_attempted", 0) < 1:
        fail("metrics did not record a provider preflight call")
    if model_metrics.get("model_lane_calls_attempted", 0) < min_ok_model_lanes:
        fail("metrics did not record enough model lane calls")


def require_no_model_evidence_failures(review: dict) -> None:
    failures = review.get("missing_or_failed_model_evidence", [])
    if failures:
        fail(f"missing_or_failed_model_evidence is not empty: {failures!r}")


def require_post_receipt(root: pathlib.Path) -> None:
    post_result = root / "review/post-result.json"
    post_error = root / "review/post-error.json"
    if not post_result.exists() and not post_error.exists():
        fail("neither post-result.json nor post-error.json exists")
    if post_result.exists():
        receipt = load_json(post_result)
        if receipt.get("status") == "skipped":
            if receipt.get("review_payload_status") != "skipped_empty_smoke":
                fail("post-result.json skipped receipt has wrong review_payload_status")
            return
        if receipt.get("status") != "ok":
            fail(f"post-result.json status expected ok or skipped, got {receipt.get('status')!r}")
        if receipt.get("review_json_valid") is not True:
            fail("post-result.json did not mark review_json_valid true")
        if receipt.get("off_diff_comment_count") not in (None, 0):
            fail("post-result.json recorded off-diff comments")
    if post_error.exists():
        receipt = load_json(post_error)
        if receipt.get("status") != "failed":
            fail(f"post-error.json status expected failed, got {receipt.get('status')!r}")
        if receipt.get("review_json_valid") is not True:
            fail("post-error.json review_json_valid is not true")
        if receipt.get("off_diff_comment_count") not in (None, 0):
            fail("post-error.json recorded off-diff comments")
        if receipt.get("failure_tolerated") is not True:
            fail("post-error.json failure_tolerated is not true")


def require_no_secret_markers(root: pathlib.Path) -> None:
    paths = [
        root / "running-summary.md",
        root / "review/review.json",
        root / "review/review.md",
        root / "review/github-review.json",
        root / "review/github-review-skip.json",
        root / "review/post-result.json",
        root / "review/post-error.json",
        root / "review/post-stdout.json",
        root / "review/post-stderr.txt",
    ]
    paths.extend((root / "lanes").glob("*.md"))
    paths.extend((root / "sensors").glob("*/ub-review-sensor-status.json"))
    paths.extend((root / "review/model").glob("**/*.json"))
    paths.extend((root / "review/provider-preflight").glob("**/*.json"))

    for path in paths:
        if not path.exists() or path.is_dir():
            continue
        text = read_text(path)
        for marker in SECRET_MARKERS:
            if marker in text:
                fail(f"secret marker {marker!r} found in {path}")


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("root", nargs="?", default="target/ub-review")
    parser.add_argument("--min-ok-model-lanes", type=int, default=0)
    parser.add_argument("--max-inline-comments", type=int)
    parser.add_argument("--require-no-model-evidence-failures", action="store_true")
    return parser.parse_args(argv[1:])


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    root = pathlib.Path(args.root)
    if not root.is_dir():
        fail(f"artifact root is not a directory: {root}")

    require_common_tree(root)
    require_summary(root)
    require_sensor_receipts(root)
    review = require_review(root, args.max_inline_comments)
    metrics = require_metrics(root, review)
    require_model_receipts(review, metrics, args.min_ok_model_lanes)
    if args.require_no_model_evidence_failures:
        require_no_model_evidence_failures(review)
    require_post_receipt(root)
    require_no_secret_markers(root)

    ok_lanes = [
        lane.get("lane")
        for lane in review.get("model_lanes", [])
        if lane.get("status") == "ok" and lane.get("lane") != "refuter"
    ]
    observations = load_json(root / "review/observations.json")
    unique_observations = load_json(root / "review/unique_observations.json")
    merged_observations = load_json(root / "review/merged_observations.json")
    dropped_observations = load_json(root / "review/dropped_observations.json")
    print(
        "Bun review artifact contract verified: "
        f"root={root} "
        f"shared_context={review['shared_context_id']} "
        f"inline_comments={len(review.get('inline_comments', []))} "
        f"observations={len(observations)} "
        f"unique_observations={len(unique_observations)} "
        f"merged_observations={len(merged_observations)} "
        f"dropped_observations={len(dropped_observations)} "
        f"ok_model_lanes={','.join(ok_lanes)}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
