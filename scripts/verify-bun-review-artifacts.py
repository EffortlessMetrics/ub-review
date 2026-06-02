#!/usr/bin/env python3
"""Verify that a UB review packet satisfies the Bun v0 artifact contract."""

from __future__ import annotations

import argparse
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
        "review/github-review.json",
    ]:
        require_file(root / path)

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
    github_review = load_json(root / "review/github-review.json")
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

    if github_review.get("event") != "COMMENT":
        fail(f"github-review.json event expected COMMENT, got {github_review.get('event')!r}")
    body = github_review.get("body")
    if not isinstance(body, str) or "## Decision" not in body:
        fail("github-review.json body is missing the review summary")
    no_standalone_approval_line(body, root / "review/github-review.json")
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


def require_metrics(root: pathlib.Path, review: dict) -> None:
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
    models = metrics.get("models")
    if not isinstance(models, dict):
        fail("metrics.models is missing")
    return metrics


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
        if receipt.get("status") != "ok":
            fail(f"post-result.json status expected ok, got {receipt.get('status')!r}")
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
    print(
        "Bun review artifact contract verified: "
        f"root={root} "
        f"shared_context={review['shared_context_id']} "
        f"inline_comments={len(review.get('inline_comments', []))} "
        f"ok_model_lanes={','.join(ok_lanes)}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
